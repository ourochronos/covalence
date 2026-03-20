#!/usr/bin/env python3
"""PDF-to-Markdown conversion sidecar service.

Converts PDF files to clean Markdown using pymupdf4llm.
Runs as a Flask HTTP service on port 8434.

API:
    POST /convert-pdf  — raw PDF bytes in body → {"markdown": "..."}
    GET  /health       — {"status": "healthy", "library": "pymupdf4llm"}
"""

import logging
import time

from flask import Flask, request, jsonify

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger("pdf-sidecar")

app = Flask(__name__)


@app.route("/health", methods=["GET"])
def health():
    """Health check endpoint."""
    return jsonify({
        "status": "healthy",
        "library": "pymupdf4llm",
    })


@app.route("/convert-pdf", methods=["POST"])
def convert_pdf():
    """Convert PDF bytes to Markdown.

    Expects raw PDF bytes in the request body.
    Returns {"markdown": "..."} on success.
    """
    import pymupdf4llm
    import tempfile
    import os

    pdf_bytes = request.get_data()
    if not pdf_bytes:
        return jsonify({"error": "Empty request body"}), 400

    # pymupdf4llm works with file paths, so write to a temp file.
    start = time.time()
    try:
        with tempfile.NamedTemporaryFile(suffix=".pdf", delete=False) as tmp:
            tmp.write(pdf_bytes)
            tmp_path = tmp.name

        md = pymupdf4llm.to_markdown(tmp_path)
        elapsed_ms = (time.time() - start) * 1000

        logger.info(
            "Converted PDF: %d bytes → %d chars markdown (%.0fms)",
            len(pdf_bytes),
            len(md),
            elapsed_ms,
        )

        return jsonify({
            "markdown": md,
            "processing_time_ms": round(elapsed_ms, 1),
        })

    except Exception as e:
        logger.error("PDF conversion failed: %s", e)
        return jsonify({"error": str(e)}), 500

    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass


if __name__ == "__main__":
    logger.info("Starting PDF conversion sidecar on port 8434...")
    app.run(host="0.0.0.0", port=8434, threaded=True)
