#!/usr/bin/env python3
"""
Claim extraction pilot — covalence#171
Runs the v2 extraction prompt against 20 selected sources from the live KB.
Saves results.json and prints a summary.
"""

import json
import os
import sys
import time
import urllib.request
import urllib.error

COVALENCE_BASE = "http://localhost:8430"
OPENAI_API_KEY = os.environ.get("OPENAI_API_KEY", "")
OPENAI_MODEL   = "gpt-4o-mini"
OUT_DIR        = os.path.dirname(os.path.abspath(__file__))

# ── 20 selected source IDs ─────────────────────────────────────────────────
# Spans domains: claims/extraction, software-architecture, pkm, distributed-systems,
#                build-systems, databases, neuroscience, reactive-programming,
#                knowledge-graphs
SELECTED_SOURCE_IDS = [
    "db32a320-de74-4c91-b915-494285d01d07",  # Claim Extraction Pilot Report (doc, claims)
    "42953910-272f-4c25-a452-1adc19e49004",  # covalence#173 Surgical Refactoring Spec (doc, sw-eng)
    "5d8bbe59-4af5-4468-b0db-234bee2016f6",  # PKM/Note-Taking Systems (web, pkm)
    "1ca95617-af01-4976-a0af-f3387370cab6",  # Claims Architecture Spec v2 (doc, sw-arch)
    "c2535e33-56a2-4f13-9d27-c27cadf9b142",  # Cache Invalidation Strategies (web, dist-sys)
    "9af22048-fecf-419e-91a0-4c1c9ccb3190",  # covalence#161 Provenance Cap & Auto-Split (doc, sw-spec)
    "d06d53ac-ca2d-4358-9b45-c7ca24f7d912",  # Incremental Compilation in Build Systems (web, build)
    "1ddd3e0e-1d74-43d9-9c19-3c0b0328660c",  # Materialized View Maintenance in Databases (web, db)
    "a7ef7b8a-0b07-4306-9bfa-131ab946a67e",  # Claims Layer Spec Blue-Green Migration (doc, sw-spec)
    "7dbed385-def7-4ab7-90d1-3141f943cf98",  # Article-to-Article Semantic Edge Inference (doc, kg)
    "027cce25-2405-42a7-a192-3e87d74fdddd",  # Claims Architecture Design Session (conv, sw-arch)
    "105c6c1b-de48-49b2-96b8-84c58d037b36",  # PKM Systems Link Suggestion (web, pkm)  ← overlaps #3
    "b7c8dfb6-a3b7-4929-98c6-73562e81e0ba",  # Synaptic Tagging and Capture STC (web, neuro) ← temporal
    "0b7dbaf7-2065-4946-bfff-912e58c4d81e",  # Incremental Build Systems (web, build)  ← overlaps #7
    "dd1fc795-6e67-4722-8d2d-d3ca00bda4cb",  # Reactive Patterns Knowledge Graph (doc, reactive)
    "cd342bbb-46ef-4a2c-b1dd-caaf1fb65b91",  # Cross-Domain Research Synthesis: 8 Domains (obs, multi)
    "09e23767-d0be-404c-8fc1-d9d17efa79f6",  # Fine-Grained Reactivity MobX Signals SolidJS (doc, reactive)
    "94711592-f9cd-45c2-b590-de165bc9486b",  # Complete Cross-Domain Research 10 Domains (obs, multi)
    "5d34dd8c-d7e0-48b3-a6ed-766615bbd47c",  # Reactive Programming & Dataflow Systems (doc, programming)
    "8f665358-8ac8-4176-b3a3-190d7dbdf578",  # Jane Street Incremental Self-Adjusting Computation (doc, cs)
]

# ── Prompts ────────────────────────────────────────────────────────────────
SYSTEM_PROMPT = """You are a precision knowledge extraction assistant for the Covalence knowledge system.
Your task is to extract discrete, verifiable, atomic claims from source documents.

## What is a "claim"?

A claim is a single, verifiable factual assertion about the world, a system, a concept, or a research finding. Claims are:

- **Atomic**: One fact per claim — no conjunctions of unrelated facts.
- **Specific**: Concrete and falsifiable, not vague summaries.
- **Self-contained**: Understandable without surrounding context (include the entity name in the claim text).
- **Verifiable**: Can in principle be confirmed true or false by consulting a source.

## What is NOT a claim?

- Vague summaries: "This paper is about caching strategies." ❌
- Procedural instructions: "To install, run `cargo build`." ❌
- Questions or hypotheticals: "Could this approach scale?" ❌
- Meta-commentary about the document itself ❌

## Entity normalization

For the `entity` field, use canonical names from this list (EXACT spelling):
Covalence, Plasmon, OpenClaw, Valence, Valence Network, Ourochronos,
PostgreSQL, Redis, Rust, RocksDB, SQLite, Bazel, Ninja, Make, CMake,
Obsidian, Roam, Logseq, Notion, Org-mode,
MobX, SolidJS, Svelte, React, Vue, Angular,
GPT-4, Claude, AnyBURL, RotatE, TransE, DistMult, ComplEx,
Wikidata, Freebase, DBpedia, YAGO, Freebase,
Excel, Pandas, NumPy, TensorFlow, PyTorch, scikit-learn,
LTP, LTPD, STC, BDNF

If no canonical entity fits, use the most natural proper noun from the text.

## Temporal claims

Flag `"temporal": true` when the claim:
- Describes version-specific behavior ("PostgreSQL 16 added...")
- Describes current/latest state as of a date ("as of 2024...")
- References benchmark numbers likely to be superseded
- Describes a roadmap item or planned feature that may change

## Output format (ONLY return this JSON — no markdown, no prose)

{
  "claims": [
    {
      "text": "A complete, self-contained atomic claim sentence.",
      "confidence": 0.85,
      "entity": "CanonicalEntityName",
      "temporal": false
    }
  ]
}

Extract 3–10 claims. Return ONLY valid JSON."""

USER_TEMPLATE = """Source title: {title}

Source content:
---
{content}
---

Extract all discrete, verifiable, atomic claims from the source above.
Return ONLY the JSON object with the `claims` array. No other text."""


# ── Helpers ────────────────────────────────────────────────────────────────

def api_get(path):
    url = COVALENCE_BASE + path
    req = urllib.request.Request(url, headers={"Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read())


def openai_chat(system, user, max_tokens=2000, retries=3):
    payload = json.dumps({
        "model": OPENAI_MODEL,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user",   "content": user},
        ],
        "max_tokens": max_tokens,
        "temperature": 0.2,
    }).encode()

    headers = {
        "Authorization": f"Bearer {OPENAI_API_KEY}",
        "Content-Type": "application/json",
    }

    for attempt in range(retries):
        try:
            req = urllib.request.Request(
                "https://api.openai.com/v1/chat/completions",
                data=payload, headers=headers, method="POST"
            )
            with urllib.request.urlopen(req, timeout=60) as resp:
                data = json.loads(resp.read())
                return data["choices"][0]["message"]["content"].strip()
        except Exception as e:
            if attempt < retries - 1:
                time.sleep(2 ** attempt)
            else:
                raise


def parse_claims_json(raw_text):
    """
    Try to parse LLM output as JSON. Handles common failure modes:
    - wrapped in ```json ... ``` fences
    - leading/trailing prose
    """
    text = raw_text.strip()

    # Strip markdown code fences
    if text.startswith("```"):
        lines = text.split("\n")
        # remove first and last fence line
        inner = []
        in_block = False
        for line in lines:
            if line.startswith("```") and not in_block:
                in_block = True
                continue
            if line.startswith("```") and in_block:
                break
            if in_block:
                inner.append(line)
        text = "\n".join(inner).strip()

    # Try direct parse
    try:
        data = json.loads(text)
        if "claims" in data and isinstance(data["claims"], list):
            return data["claims"], None
        return None, f"Parsed JSON but no 'claims' list: keys={list(data.keys())}"
    except json.JSONDecodeError as e:
        # Try extracting JSON object from surrounding text
        start = text.find("{")
        end   = text.rfind("}") + 1
        if start >= 0 and end > start:
            try:
                data = json.loads(text[start:end])
                if "claims" in data:
                    return data["claims"], None
            except Exception:
                pass
        return None, f"JSON parse error: {e} | raw_len={len(raw_text)}"


def truncate_content(content, max_chars=12000):
    """Truncate very long sources to avoid token limits."""
    if len(content) <= max_chars:
        return content
    return content[:max_chars] + f"\n\n[... truncated from {len(content)} chars ...]"


# ── Main extraction loop ───────────────────────────────────────────────────

def main():
    if not OPENAI_API_KEY:
        print("ERROR: OPENAI_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    results = []

    for i, source_id in enumerate(SELECTED_SOURCE_IDS):
        print(f"\n[{i+1:2d}/20] Fetching source {source_id[:8]}...", flush=True)

        # 1. Fetch source from Covalence API
        try:
            resp = api_get(f"/sources/{source_id}")
            # API wraps single-source responses in {"data": {...}}
            source = resp.get("data", resp) if isinstance(resp, dict) else resp
        except Exception as e:
            print(f"       ⚠ API fetch failed: {e}")
            results.append({
                "source_id":    source_id,
                "source_title": "(unknown — fetch failed)",
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"API fetch failed: {e}",
            })
            continue

        title   = source.get("title", "(untitled)")
        content = source.get("content", "")
        print(f"       title: {title[:70]}")
        print(f"       content_len: {len(content)} chars")

        if len(content) < 50:
            print("       ⚠ Content too short — skipping")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"Content too short ({len(content)} chars)",
            })
            continue

        # 2. Build prompt
        user_prompt = USER_TEMPLATE.format(
            title=title,
            content=truncate_content(content),
        )

        # 3. Call LLM
        try:
            raw_response = openai_chat(SYSTEM_PROMPT, user_prompt)
        except Exception as e:
            print(f"       ⚠ LLM call failed: {e}")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"LLM call failed: {e}",
            })
            continue

        # 4. Parse JSON
        claims, err = parse_claims_json(raw_response)
        if err:
            print(f"       ⚠ Parse error: {err}")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"JSON parse error: {err} | raw_snippet={raw_response[:200]}",
            })
            continue

        print(f"       ✓ {len(claims)} claims extracted")
        for c in claims[:3]:
            conf = c.get("confidence","?")
            ent  = c.get("entity","?")
            temp = "⏰" if c.get("temporal") else ""
            print(f"         [{conf}]{temp} [{ent}] {c.get('text','')[:80]}")
        if len(claims) > 3:
            print(f"         ... and {len(claims)-3} more")

        results.append({
            "source_id":    source_id,
            "source_title": title,
            "claims":       claims,
            "extraction_ok": True,
            "notes":        "",
        })

        # Small delay to avoid rate limiting
        time.sleep(0.5)

    # ── Save results ─────────────────────────────────────────────────────
    out_path = os.path.join(OUT_DIR, "results.json")
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\n✅ Saved {out_path}")

    # Summary
    ok      = [r for r in results if r["extraction_ok"]]
    failed  = [r for r in results if not r["extraction_ok"]]
    total_c = sum(len(r["claims"]) for r in ok)
    counts  = [len(r["claims"]) for r in ok]
    temporal = sum(1 for r in ok for c in r["claims"] if c.get("temporal"))

    print(f"\n── Extraction Summary ──────────────────")
    print(f"  Sources processed:  {len(results)}")
    print(f"  Successful:         {len(ok)}")
    print(f"  Failed:             {len(failed)}")
    print(f"  Total claims:       {total_c}")
    if counts:
        print(f"  Claims per source:  min={min(counts)} max={max(counts)} avg={total_c/len(ok):.1f}")
    print(f"  Temporal claims:    {temporal} ({100*temporal/total_c:.0f}% of total)" if total_c else "")

    if failed:
        print("\n  Failed sources:")
        for r in failed:
            print(f"    {r['source_id'][:8]} — {r['notes'][:80]}")

    return results


if __name__ == "__main__":
    main()
