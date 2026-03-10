"""
Unified extraction sidecar — serves all local models behind a single FastAPI service.

Models:
  - Fastcoref (coreference resolution)
  - GLiNER2 (named entity recognition)
  - NuExtract-1.5-tiny (relationship extraction)

All models lazy-loaded on first request. Run with:
  cd sidecar/extraction && ../../.venv-312/bin/uvicorn server:app --host 127.0.0.1 --port 8433
"""

import json
import logging
from typing import Optional

import torch
from fastapi import FastAPI
from pydantic import BaseModel, Field

logger = logging.getLogger("extraction-sidecar")

app = FastAPI(title="Covalence Extraction Sidecar", version="0.1.0")

# ---------------------------------------------------------------------------
# Lazy model holders
# ---------------------------------------------------------------------------
_coref_model = None
_gliner_model = None
_nuextract_model = None
_nuextract_tokenizer = None


def get_coref():
    global _coref_model
    if _coref_model is None:
        from fastcoref import FCoref
        logger.info("Loading fastcoref (biu-nlp/f-coref)...")
        _coref_model = FCoref(device="cpu")
        logger.info("Fastcoref loaded.")
    return _coref_model


def get_gliner():
    global _gliner_model
    if _gliner_model is None:
        from gliner import GLiNER
        logger.info("Loading GLiNER (urchade/gliner_medium-v2.1)...")
        _gliner_model = GLiNER.from_pretrained("urchade/gliner_medium-v2.1")
        logger.info("GLiNER loaded.")
    return _gliner_model


def get_nuextract():
    global _nuextract_model, _nuextract_tokenizer
    if _nuextract_model is None:
        from transformers import AutoModelForCausalLM, AutoTokenizer
        model_name = "numind/NuExtract-1.5-tiny"
        logger.info(f"Loading NuExtract ({model_name})...")
        _nuextract_tokenizer = AutoTokenizer.from_pretrained(
            model_name, trust_remote_code=True
        )
        _nuextract_model = AutoModelForCausalLM.from_pretrained(
            model_name, trust_remote_code=True, dtype=torch.float32
        )
        _nuextract_model.eval()
        logger.info("NuExtract loaded.")
    return _nuextract_model, _nuextract_tokenizer


# ---------------------------------------------------------------------------
# Request / Response models
# ---------------------------------------------------------------------------
class CorefRequest(BaseModel):
    texts: list[str]


class CorefResponse(BaseModel):
    results: list[dict]


class NerRequest(BaseModel):
    text: str
    labels: list[str] = Field(
        default=["person", "organization", "technology", "concept",
                 "event", "location", "algorithm"]
    )
    threshold: float = 0.4


class NerResponse(BaseModel):
    entities: list[dict]


class RelRequest(BaseModel):
    text: str
    entities: list[dict] = Field(default=[])
    schema_template: Optional[str] = None


class RelResponse(BaseModel):
    relationships: list[dict]


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------
@app.get("/health")
def health():
    return {
        "status": "ok",
        "models": {
            "coref": _coref_model is not None,
            "gliner": _gliner_model is not None,
            "nuextract": _nuextract_model is not None,
        },
    }


@app.post("/coref", response_model=CorefResponse)
def coreference(req: CorefRequest):
    model = get_coref()
    preds = model.predict(texts=req.texts)
    results = []
    for text, pred in zip(req.texts, preds):
        clusters = pred.get_clusters(as_strings=True)
        resolved = text
        for cluster in clusters:
            antecedent = cluster[0]
            for mention in cluster[1:]:
                if len(mention) <= 5:
                    resolved = resolved.replace(mention, antecedent, 1)
        results.append({
            "original": text,
            "resolved": resolved,
            "clusters": clusters,
        })
    return CorefResponse(results=results)


@app.post("/ner", response_model=NerResponse)
def named_entity_recognition(req: NerRequest):
    model = get_gliner()
    entities = model.predict_entities(req.text, req.labels, threshold=req.threshold)
    return NerResponse(
        entities=[
            {
                "text": e["text"],
                "label": e["label"],
                "score": round(e["score"], 4),
                "start": e.get("start"),
                "end": e.get("end"),
            }
            for e in entities
        ]
    )


@app.post("/relationships", response_model=RelResponse)
def relationship_extraction(req: RelRequest):
    model, tokenizer = get_nuextract()

    template = req.schema_template or json.dumps({
        "relationships": [{
            "source_entity": "",
            "target_entity": "",
            "relationship_type": "",
            "description": "",
        }]
    })

    prompt = f"<|input|>\n### Template:\n{template}\n### Text:\n{req.text}\n\n<|output|>"

    input_ids = tokenizer(prompt, return_tensors="pt", truncation=True, max_length=4000)
    with torch.no_grad():
        output = model.generate(
            **input_ids, max_new_tokens=1000, temperature=0.0, do_sample=False
        )

    result = tokenizer.decode(output[0], skip_special_tokens=True)
    if "<|output|>" in result:
        result = result.split("<|output|>")[-1].strip()

    try:
        parsed = json.loads(result)
        relationships = parsed.get("relationships", [])
    except json.JSONDecodeError:
        logger.warning(f"Failed to parse NuExtract output: {result[:200]}")
        relationships = []

    return RelResponse(relationships=relationships)


@app.post("/extract")
def full_extract(req: NerRequest):
    """Convenience endpoint: coref → NER → relationships in one call."""
    # Step 1: Coreference resolution
    coref_model = get_coref()
    preds = coref_model.predict(texts=[req.text])
    pred = preds[0]
    clusters = pred.get_clusters(as_strings=True)

    # Simple string-based resolution: replace pronouns with antecedents
    resolved = req.text
    for cluster in clusters:
        antecedent = cluster[0]
        for mention in cluster[1:]:
            # Only replace short pronouns, not longer noun phrases
            if len(mention) <= 5:
                resolved = resolved.replace(mention, antecedent, 1)

    # Step 2: NER on resolved text
    gliner = get_gliner()
    entities = gliner.predict_entities(resolved, req.labels, threshold=req.threshold)

    # Step 3: Relationship extraction on resolved text
    nuextract_model, tokenizer = get_nuextract()
    template = json.dumps({
        "relationships": [{
            "source_entity": "",
            "target_entity": "",
            "relationship_type": "",
            "description": "",
        }]
    })

    prompt = f"<|input|>\n### Template:\n{template}\n### Text:\n{resolved}\n\n<|output|>"
    input_ids = tokenizer(prompt, return_tensors="pt", truncation=True, max_length=4000)
    with torch.no_grad():
        output = nuextract_model.generate(
            **input_ids, max_new_tokens=1000, temperature=0.0, do_sample=False
        )

    result = tokenizer.decode(output[0], skip_special_tokens=True)
    if "<|output|>" in result:
        result = result.split("<|output|>")[-1].strip()

    try:
        parsed = json.loads(result)
        relationships = parsed.get("relationships", [])
    except json.JSONDecodeError:
        relationships = []

    return {
        "resolved_text": resolved,
        "coref_clusters": clusters,
        "entities": [
            {"text": e["text"], "label": e["label"], "score": round(e["score"], 4)}
            for e in entities
        ],
        "relationships": relationships,
    }
