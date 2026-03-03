"""
covalence.client — High-level convenience client for the Covalence Knowledge Engine.

The generated low-level API classes (SourcesApi, ArticlesApi, …) talk directly
to the wire and parse response bodies into typed model objects.  However,
every Covalence endpoint wraps its response in ``{"data": <payload>}``.
Because that envelope is not reflected in the utoipa-generated OpenAPI spec
the generated client's deserialization doesn't unwrap it automatically.

This module provides ``CovalenceClient``: a thin hand-rolled wrapper that:
  1. Makes HTTP calls with ``urllib.request`` (zero extra deps)
  2. Unwraps the ``data`` envelope transparently
  3. Exposes a clean Pythonic API covering the five endpoint groups

Usage::

    from covalence.client import CovalenceClient

    c = CovalenceClient("http://localhost:8430")

    src = c.ingest_source("Some raw text", source_type="observation")
    results = c.search("epistemic model", limit=5)
    mem = c.store_memory("Agent observed X", importance=0.8)
    stats = c.admin_stats()

All methods return plain dicts / lists.  If you need typed objects, use the
low-level generated API classes from ``covalence.api`` instead.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any, Dict, List, Optional, Union


class CovalenceError(Exception):
    """Raised when the Covalence engine returns a non-2xx response."""

    def __init__(self, status: int, body: str) -> None:
        self.status = status
        self.body = body
        super().__init__(f"HTTP {status}: {body}")


class CovalenceClient:
    """
    Zero-dependency convenience client for the Covalence Knowledge Engine.

    :param base_url: Engine base URL, e.g. ``http://localhost:8430``.
    :param timeout:  Request timeout in seconds (default: 30).
    """

    def __init__(self, base_url: str = "http://localhost:8430", timeout: int = 30) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

    # ── Internal helpers ───────────────────────────────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        body: Optional[Dict[str, Any]] = None,
    ) -> Any:
        url = self.base_url + path
        data: Optional[bytes] = None
        headers = {"Content-Type": "application/json", "Accept": "application/json"}

        if body is not None:
            data = json.dumps(body).encode()

        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read().decode()
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode()
            raise CovalenceError(exc.code, raw) from exc

        if not raw.strip():
            return None  # e.g. 204 No Content

        payload = json.loads(raw)
        # Unwrap {"data": …} envelope if present
        if isinstance(payload, dict) and "data" in payload:
            return payload["data"]
        return payload

    def _get(self, path: str) -> Any:
        return self._request("GET", path)

    def _post(self, path: str, body: Dict[str, Any]) -> Any:
        return self._request("POST", path, body)

    def _delete(self, path: str) -> Any:
        return self._request("DELETE", path)

    # ── Sources ────────────────────────────────────────────────────────────────

    def ingest_source(
        self,
        content: str,
        *,
        title: Optional[str] = None,
        source_type: Optional[str] = None,
        reliability: Optional[float] = None,
        session_id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
        capture_method: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Ingest a new raw source. Returns the created SourceResponse dict."""
        body: Dict[str, Any] = {"content": content, "metadata": metadata or {}}
        if title is not None:
            body["title"] = title
        if source_type is not None:
            body["source_type"] = source_type
        if reliability is not None:
            body["reliability"] = reliability
        if session_id is not None:
            body["session_id"] = session_id
        if capture_method is not None:
            body["capture_method"] = capture_method
        return self._post("/sources", body)

    def get_source(self, source_id: str) -> Dict[str, Any]:
        """Retrieve a source by UUID."""
        return self._get(f"/sources/{source_id}")

    def delete_source(self, source_id: str) -> None:
        """Delete a source by UUID."""
        self._delete(f"/sources/{source_id}")

    # ── Articles ───────────────────────────────────────────────────────────────

    def compile_article(
        self,
        source_ids: List[str],
        *,
        title_hint: Optional[str] = None,
        compilation_focus: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Enqueue a compilation job. Returns CompileJobResponse dict."""
        body: Dict[str, Any] = {"source_ids": source_ids}
        if title_hint is not None:
            body["title_hint"] = title_hint
        if compilation_focus is not None:
            body["compilation_focus"] = compilation_focus
        return self._post("/articles/compile", body)

    def get_article(self, article_id: str) -> Dict[str, Any]:
        """Retrieve an article by UUID."""
        return self._get(f"/articles/{article_id}")

    def merge_articles(
        self, article_id_a: str, article_id_b: str
    ) -> Dict[str, Any]:
        """Merge two articles. Returns merged ArticleResponse dict."""
        return self._post("/articles/merge", {"article_id_a": article_id_a, "article_id_b": article_id_b})

    # ── Search ─────────────────────────────────────────────────────────────────

    def search(
        self,
        query: str,
        *,
        limit: int = 10,
        strategy: Optional[str] = None,
        mode: Optional[str] = None,
        node_types: Optional[List[str]] = None,
        min_score: Optional[float] = None,
        domain_path: Optional[List[str]] = None,
        session_id: Optional[str] = None,
        **kwargs: Any,
    ) -> List[Dict[str, Any]]:
        """
        Three-dimensional knowledge search.

        Returns a list of SearchResult dicts ordered by score descending.
        """
        body: Dict[str, Any] = {"query": query, "limit": limit}
        if strategy is not None:
            body["strategy"] = strategy
        if mode is not None:
            body["mode"] = mode
        if node_types is not None:
            body["node_types"] = node_types
        if min_score is not None:
            body["min_score"] = min_score
        if domain_path is not None:
            body["domain_path"] = domain_path
        if session_id is not None:
            body["session_id"] = session_id
        body.update(kwargs)
        return self._post("/search", body)

    # ── Memory ─────────────────────────────────────────────────────────────────

    def store_memory(
        self,
        content: str,
        *,
        context: Optional[str] = None,
        importance: float = 0.5,
        tags: Optional[List[str]] = None,
        supersedes_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Store a memory. Returns the Memory dict."""
        body: Dict[str, Any] = {"content": content, "importance": importance}
        if context is not None:
            body["context"] = context
        if tags is not None:
            body["tags"] = tags
        if supersedes_id is not None:
            body["supersedes_id"] = supersedes_id
        return self._post("/memory", body)

    def recall_memory(
        self,
        query: str,
        *,
        limit: int = 5,
        min_confidence: Optional[float] = None,
        tags: Optional[List[str]] = None,
        context_prefix: Optional[str] = None,
        since: Optional[str] = None,
    ) -> List[Dict[str, Any]]:
        """Recall memories. Returns list of Memory dicts."""
        body: Dict[str, Any] = {"query": query, "limit": limit}
        if min_confidence is not None:
            body["min_confidence"] = min_confidence
        if tags is not None:
            body["tags"] = tags
        if context_prefix is not None:
            body["context_prefix"] = context_prefix
        if since is not None:
            body["since"] = since
        return self._post("/memory/search", body)

    # ── Admin ──────────────────────────────────────────────────────────────────

    def admin_stats(self) -> Dict[str, Any]:
        """Return system statistics dict."""
        return self._get("/admin/stats")

    def admin_maintenance(
        self,
        *,
        recompute_scores: Optional[bool] = None,
        process_queue: Optional[bool] = None,
        evict_if_over_capacity: Optional[bool] = None,
        evict_count: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Run maintenance operations. Returns MaintenanceResponse dict."""
        body: Dict[str, Any] = {}
        if recompute_scores is not None:
            body["recompute_scores"] = recompute_scores
        if process_queue is not None:
            body["process_queue"] = process_queue
        if evict_if_over_capacity is not None:
            body["evict_if_over_capacity"] = evict_if_over_capacity
        if evict_count is not None:
            body["evict_count"] = evict_count
        return self._post("/admin/maintenance", body)

    # ── Convenience ────────────────────────────────────────────────────────────

    def health(self) -> bool:
        """Return True if the engine is up and reachable (pings admin/stats)."""
        try:
            self._get("/admin/stats")
            return True
        except CovalenceError:
            return False
        except Exception:
            return False

    def __repr__(self) -> str:
        return f"CovalenceClient(base_url={self.base_url!r})"
