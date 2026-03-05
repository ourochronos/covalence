#!/usr/bin/env python3
"""
Claim extraction staging validation — covalence#171
Runs the v3 extraction prompt against 100 selected sources from the live KB.
Pre-conditions from P0-1 pilot are now met:
  1. Full 102-entity canonical list injected
  2. List-expansion rule added
  3. Self-containedness instruction strengthened
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
SOURCES_FILE   = "/tmp/staging_sources_100.json"

# ── Build system prompt from entity-normalization.json ────────────────────
ENTITY_JSON = os.path.join(os.path.dirname(__file__), "..", "entity-normalization.json")

def build_entity_list():
    with open(ENTITY_JSON) as f:
        entities = json.load(f)
    lines = []
    for canonical, aliases in entities.items():
        if aliases:
            alias_str = ", ".join(aliases[:10])
            if len(aliases) > 10:
                alias_str += f" (+{len(aliases)-10} more)"
            lines.append(f"- {canonical} (aliases: {alias_str})")
        else:
            lines.append(f"- {canonical}")
    return "\n".join(lines)

ENTITY_LIST = build_entity_list()

SYSTEM_PROMPT = f"""You are a precision knowledge extraction assistant for the Covalence knowledge system.
Your task is to extract discrete, verifiable, atomic claims from source documents.

## What is a "claim"?

A claim is a single, verifiable factual assertion about the world, a system, a concept, or a research finding. Claims are:

- **Atomic**: One fact per claim — no conjunctions of unrelated facts.
- **Specific**: Concrete and falsifiable, not vague summaries.
- **Self-contained**: Understandable without the surrounding context. Every claim MUST include the entity name if there is a primary entity for the source.
- **Verifiable**: Can in principle be confirmed true or false by consulting a source.

## What is NOT a claim?

- Vague summaries: "This paper is about caching strategies." ❌
- Procedural instructions: "To install, run `cargo build`." ❌
- Questions or hypotheticals: "Could this approach scale to millions?" ❌
- Meta-commentary: "The author argues that..." ❌ (state the argument directly instead)
- Tautologies: "Incremental builds are incremental." ❌

## Entity normalization

For the `entity` field, use the canonical entity names from the following list when applicable.
Use EXACT canonical spelling. If no canonical entity applies, use the most natural proper noun.

### Canonical entities (full list — 102 entries — use canonical spelling only):
{ENTITY_LIST}

## List expansion

When a source sentence enumerates multiple items in a single assertion (e.g., "X supports A, B, and C"), produce one claim per item rather than one compound claim. Example:
  Input: "The system is grounded in five frameworks: FEP, AGM, Stigmergy, Pearl's Causal Hierarchy, and CLS."
  Output:
    - "The system is grounded in the Free Energy Principle (FEP)."
    - "The system is grounded in Belief Revision (AGM)."
    ... (one per item)

Apply this only when the list contains 2+ distinct entities or concepts. Do not split naturally compound facts (e.g., "X supports both read and write operations" → keep as one claim).

## Temporal claims

Flag a claim as "temporal": true when:
- It describes a version-specific behavior (e.g. "PostgreSQL 16 added...")
- It describes current/latest state ("currently supports", "as of 2024...")
- It describes a finding from a specific dated study
- It is about a project spec/feature that may change (roadmap items, planned features)
- It references benchmark numbers that may be superseded
- It describes a Covalence architectural decision, specification, or roadmap item (specs are living documents and may change)

## Output format

Return ONLY valid JSON. No markdown, no prose, no explanation.

{{
  "claims": [
    {{
      "text": "A complete, self-contained atomic claim sentence.",
      "confidence": 0.85,
      "entity": "CanonicalEntityName",
      "temporal": false
    }}
  ]
}}

Extract 3-10 claims per source. For dense technical documents, extract up to 15 claims if warranted. If a source yields fewer than 3 meaningful claims, return fewer. Always return valid JSON."""

USER_TEMPLATE = """Source title: {title}

Source content:
---
{content}
---

Extract all discrete, verifiable, atomic claims from the source above.
Return ONLY the JSON object with the `claims` array. No other text."""


# ── Helpers ────────────────────────────────────────────────────────────────

def openai_chat(system, user, max_tokens=2500, retries=3):
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
            with urllib.request.urlopen(req, timeout=90) as resp:
                data = json.loads(resp.read())
                return data["choices"][0]["message"]["content"].strip()
        except Exception as e:
            if attempt < retries - 1:
                wait = 2 ** attempt
                print(f"       ⚠ Attempt {attempt+1} failed: {e} — retrying in {wait}s")
                time.sleep(wait)
            else:
                raise


def parse_claims_json(raw_text):
    text = raw_text.strip()
    if text.startswith("```"):
        lines = text.split("\n")
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

    try:
        data = json.loads(text)
        if "claims" in data and isinstance(data["claims"], list):
            return data["claims"], None
        return None, f"Parsed JSON but no 'claims' list: keys={list(data.keys())}"
    except json.JSONDecodeError as e:
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


def truncate_content(content, max_chars=14000):
    if len(content) <= max_chars:
        return content
    return content[:max_chars] + f"\n\n[... truncated from {len(content)} chars ...]"


# ── Main extraction loop ───────────────────────────────────────────────────

def main():
    if not OPENAI_API_KEY:
        print("ERROR: OPENAI_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    with open(SOURCES_FILE) as f:
        sources = json.load(f)

    print(f"Loaded {len(sources)} sources for staging extraction (v3 prompt)")
    print(f"Model: {OPENAI_MODEL}")
    print(f"Entity list: 102 canonical entities")
    print()

    results = []
    total = len(sources)

    for i, source in enumerate(sources):
        source_id = source.get("id", "unknown")
        title     = source.get("title", "(untitled)")
        content   = source.get("content", "")
        domain    = source.get("_domain", "?")

        print(f"[{i+1:3d}/{total}] {source_id[:8]} [{domain}]")
        print(f"       title: {title[:65]}")
        print(f"       content_len: {len(content)} chars")

        if len(content) < 200:
            print("       ⚠ Content < 200 chars — skipping")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "domain":       domain,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"Content too short ({len(content)} chars)",
            })
            continue

        user_prompt = USER_TEMPLATE.format(
            title=title,
            content=truncate_content(content),
        )

        try:
            raw_response = openai_chat(SYSTEM_PROMPT, user_prompt)
        except Exception as e:
            print(f"       ⚠ LLM call failed: {e}")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "domain":       domain,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"LLM call failed: {e}",
            })
            time.sleep(1)
            continue

        claims, err = parse_claims_json(raw_response)
        if err:
            print(f"       ⚠ Parse error: {err}")
            results.append({
                "source_id":    source_id,
                "source_title": title,
                "domain":       domain,
                "claims":       [],
                "extraction_ok": False,
                "notes":        f"JSON parse error: {err} | raw_snippet={raw_response[:200]}",
            })
            continue

        print(f"       ✓ {len(claims)} claims")
        for c in claims[:2]:
            conf = c.get("confidence", "?")
            ent  = c.get("entity", "?")
            temp = "⏰" if c.get("temporal") else ""
            print(f"         [{conf}]{temp} [{ent}] {c.get('text','')[:72]}")
        if len(claims) > 2:
            print(f"         ... and {len(claims)-2} more")

        results.append({
            "source_id":    source_id,
            "source_title": title,
            "domain":       domain,
            "claims":       claims,
            "extraction_ok": True,
            "notes":        "",
        })

        # Rate limit: ~2 req/sec
        time.sleep(0.5)

    # ── Save results ──────────────────────────────────────────────────────
    out_path = os.path.join(OUT_DIR, "results.json")
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\n✅ Saved {out_path}")

    # ── Summary ───────────────────────────────────────────────────────────
    ok      = [r for r in results if r["extraction_ok"]]
    failed  = [r for r in results if not r["extraction_ok"]]
    total_c = sum(len(r["claims"]) for r in ok)
    counts  = [len(r["claims"]) for r in ok]
    temporal = sum(1 for r in ok for c in r["claims"] if c.get("temporal"))
    entities_all = [c.get("entity") for r in ok for c in r["claims"]]
    null_ent = sum(1 for e in entities_all if not e)

    print(f"\n── Extraction Summary (v3 prompt) ──────────────────────────")
    print(f"  Sources processed:  {len(results)}")
    print(f"  Successful:         {len(ok)}")
    print(f"  Failed:             {len(failed)}")
    print(f"  Total claims:       {total_c}")
    if counts:
        import statistics
        print(f"  Claims per source:  min={min(counts)} max={max(counts)} avg={total_c/len(ok):.1f} median={statistics.median(counts):.0f}")
    if total_c:
        print(f"  Temporal claims:    {temporal} ({100*temporal/total_c:.1f}%)")
        print(f"  Null entity:        {null_ent} ({100*null_ent/total_c:.1f}%)")
        conf_vals = [c.get("confidence",0) for r in ok for c in r["claims"]]
        print(f"  Confidence ≥0.9:    {sum(1 for c in conf_vals if c>=0.9)} ({100*sum(1 for c in conf_vals if c>=0.9)/total_c:.1f}%)")
        print(f"  Confidence ≥0.85:   {sum(1 for c in conf_vals if c>=0.85)} ({100*sum(1 for c in conf_vals if c>=0.85)/total_c:.1f}%)")
        print(f"  Confidence ≥0.8:    {sum(1 for c in conf_vals if c>=0.8)} ({100*sum(1 for c in conf_vals if c>=0.8)/total_c:.1f}%)")

    if failed:
        print(f"\n  Failed sources:")
        for r in failed:
            print(f"    {r['source_id'][:8]} — {r['notes'][:80]}")


if __name__ == "__main__":
    main()
