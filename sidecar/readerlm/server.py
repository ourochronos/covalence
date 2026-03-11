"""ReaderLM-v2 sidecar — converts HTML to clean Markdown via MLX.

Run:
    cd sidecar/readerlm
    ../../.venv/bin/uvicorn server:app --host 127.0.0.1 --port 8432

Requires: mlx-lm, fastapi, uvicorn (install into project venv).
"""

import asyncio
import logging
import time

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

logger = logging.getLogger("readerlm")
logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

app = FastAPI(title="ReaderLM-v2 Sidecar", version="0.1.0")

# Serialize all inference requests — MLX Metal command buffers
# cannot handle concurrent encoding. This lock ensures only one
# generate() call is in-flight at a time.
_inference_lock = asyncio.Lock()

# Lazy-load model on first request to keep startup fast.
_model = None
_tokenizer = None


def _load_model():
    global _model, _tokenizer
    if _model is not None:
        return
    logger.info("Loading mlx-community/jinaai-ReaderLM-v2 ...")
    t0 = time.monotonic()
    from mlx_lm import load

    _model, _tokenizer = load("mlx-community/jinaai-ReaderLM-v2")
    elapsed = time.monotonic() - t0
    logger.info("Model loaded in %.1fs", elapsed)


class ConvertRequest(BaseModel):
    html: str
    max_tokens: int = 8192


class ConvertResponse(BaseModel):
    markdown: str
    elapsed_ms: int


@app.get("/health")
def health():
    return {"status": "ok", "model": "jinaai-ReaderLM-v2"}


@app.post("/convert", response_model=ConvertResponse)
async def convert(req: ConvertRequest):
    if not req.html.strip():
        return ConvertResponse(markdown="", elapsed_ms=0)

    async with _inference_lock:
        try:
            _load_model()
        except Exception as e:
            logger.exception("Failed to load model")
            raise HTTPException(status_code=503, detail=f"model load failed: {e}") from e

        import mlx.core as mx
        from mlx_lm import generate

        # ReaderLM-v2 expects the HTML wrapped in a chat-style prompt.
        messages = [{"role": "user", "content": req.html}]
        prompt = _tokenizer.apply_chat_template(
            messages, tokenize=False, add_generation_prompt=True
        )

        t0 = time.monotonic()
        markdown = generate(
            _model,
            _tokenizer,
            prompt=prompt,
            max_tokens=req.max_tokens,
            verbose=False,
        )
        # Ensure Metal command buffers are fully flushed before
        # releasing the lock. Without this, the next generate()
        # call can collide with in-flight GPU work and trigger:
        # "A command encoder is already encoding to this command buffer"
        mx.synchronize()
        elapsed_ms = int((time.monotonic() - t0) * 1000)

    logger.info(
        "Converted %d chars HTML -> %d chars MD in %dms",
        len(req.html),
        len(markdown),
        elapsed_ms,
    )
    return ConvertResponse(markdown=markdown, elapsed_ms=elapsed_ms)
