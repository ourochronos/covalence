import multiprocessing
multiprocessing.set_start_method("fork", force=True)

import time, sys, os
os.environ["PDFTEXT_CPU_WORKERS"] = "1"

if __name__ == "__main__":
    print("Loading marker...")
    t0 = time.time()
    from marker.converters.pdf import PdfConverter
    from marker.models import create_model_dict
    print(f"Import: {time.time()-t0:.1f}s")

    print("Loading models...")
    t0 = time.time()
    models = create_model_dict()
    print(f"Models loaded: {time.time()-t0:.1f}s")

    converter = PdfConverter(artifact_dict=models)

    pdf_path = sys.argv[1] if len(sys.argv) > 1 else "/Users/zonk1024/projects/covalence/.test-fixtures/graphrag-paper.pdf"

    print(f"Converting {pdf_path}...")
    t0 = time.time()
    result = converter(pdf_path)
    elapsed = time.time() - t0

    md = result.markdown
    print(f"Conversion: {elapsed:.1f}s")
    print(f"Output: {len(md)} chars, {len(md.splitlines())} lines")
    print()
    print("=== FIRST 2000 CHARS ===")
    print(md[:2000])
    print()
    print("=== LAST 500 CHARS ===")
    print(md[-500:])
