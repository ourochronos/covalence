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
        clusters_str = pred.get_clusters(as_strings=True)
        clusters_spans = pred.get_clusters(as_strings=False)
        resolved, mutations = _resolve_with_mutations(text, clusters_str, clusters_spans)
        results.append({
            "original": text,
            "resolved": resolved,
            "clusters": clusters_str,
            "mutations": mutations,
        })
    return CorefResponse(results=results)


def _resolve_with_mutations(
    text: str,
    clusters_str: list[list[str]],
    clusters_spans: list[list[tuple[int, int]]],
) -> tuple[str, list[dict]]:
    """Replace anaphoric mentions with antecedents using exact character spans.

    Returns (resolved_text, mutations) where each mutation records:
      - canonical_start/end: byte offsets in the original text
      - mutated_start/end: byte offsets in the resolved text
      - canonical_token: the original mention (e.g. "He")
      - mutated_token: the replacement (e.g. "Einstein")

    This is the data the Rust engine needs to build the offset
    projection ledger for reverse-projecting entity byte spans back
    to canonical source positions.
    """
    # Build list of (char_start, char_end, antecedent) replacements,
    # sorted by position. We skip the first mention in each cluster
    # (the antecedent itself) and only replace shorter mentions
    # (pronouns, abbreviated references).
    replacements = []
    for str_cluster, span_cluster in zip(clusters_str, clusters_spans):
        if len(str_cluster) < 2 or len(span_cluster) < 2:
            continue
        antecedent = str_cluster[0]
        for mention_str, (char_start, char_end) in zip(str_cluster[1:], span_cluster[1:]):
            # Only replace mentions shorter than the antecedent
            # (pronouns like "he", "it", "they", possessives, etc.)
            if len(mention_str) < len(antecedent):
                replacements.append((char_start, char_end, antecedent))

    if not replacements:
        return text, []

    # Sort by position (ascending) and deduplicate overlapping spans
    # (keep the first one encountered).
    replacements.sort(key=lambda r: r[0])
    deduped = []
    last_end = -1
    for start, end, antecedent in replacements:
        if start >= last_end:
            deduped.append((start, end, antecedent))
            last_end = end
    replacements = deduped

    # Convert character offsets to byte offsets in the original text.
    # Python strings are UTF-8 internally when encoded.
    text_bytes = text.encode("utf-8")
    char_to_byte = _build_char_to_byte_map(text)

    mutations = []
    resolved_parts = []
    # Track byte position in the resolved output.
    mutated_byte_pos = 0
    prev_byte_end = 0

    for char_start, char_end, antecedent in replacements:
        byte_start = char_to_byte[char_start]
        byte_end = char_to_byte[char_end] if char_end < len(char_to_byte) else len(text_bytes)
        canonical_token = text_bytes[byte_start:byte_end].decode("utf-8", errors="replace")
        mutated_token = antecedent

        # Append unchanged text before this replacement.
        unchanged = text_bytes[prev_byte_end:byte_start]
        resolved_parts.append(unchanged)
        mutated_byte_pos += len(unchanged)

        # Append replacement.
        replacement_bytes = mutated_token.encode("utf-8")
        resolved_parts.append(replacement_bytes)

        mutations.append({
            "canonical_start": byte_start,
            "canonical_end": byte_end,
            "mutated_start": mutated_byte_pos,
            "mutated_end": mutated_byte_pos + len(replacement_bytes),
            "canonical_token": canonical_token,
            "mutated_token": mutated_token,
        })

        mutated_byte_pos += len(replacement_bytes)
        prev_byte_end = byte_end

    # Append remaining text after last replacement.
    resolved_parts.append(text_bytes[prev_byte_end:])
    resolved_text = b"".join(resolved_parts).decode("utf-8", errors="replace")

    return resolved_text, mutations


def _build_char_to_byte_map(text: str) -> list[int]:
    """Map character index → byte offset in UTF-8 encoding."""
    result = []
    byte_pos = 0
    for ch in text:
        result.append(byte_pos)
        byte_pos += len(ch.encode("utf-8"))
    result.append(byte_pos)  # sentinel for end-of-string
    return result


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

    # Span-based resolution with mutation tracking
    clusters_spans = pred.get_clusters(as_strings=False)
    resolved, mutations = _resolve_with_mutations(req.text, clusters, clusters_spans)

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
        "mutations": mutations,
        "entities": [
            {"text": e["text"], "label": e["label"], "score": round(e["score"], 4)}
            for e in entities
        ],
        "relationships": relationships,
    }
