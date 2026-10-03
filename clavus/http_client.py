"""
Minimal HTTP client using stdlib only (urllib.request).

Replaces httpx dependency throughout Clavus.
urllib.request equivalents:
  - httpx.get(url, params={...}, timeout=)  → http_get(url, params, timeout)
  - httpx.post(url, json={...}, params={...}, timeout=) → http_post(url, json, params, timeout)
  - response.json() → json.loads(response.read().decode())

urllib error types:
  - urllib.error.HTTPError: raised on HTTP 4xx/5xx (status in .code)
  - urllib.error.URLError: wraps socket errors (ConnectError, DNS failure)
  - socket.timeout: raised on read timeout
"""
from __future__ import annotations

import json
import socket
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Optional


def _build_url(url: str, params: Optional[dict] = None) -> str:
    """Append query params to URL."""
    if not params:
        return url
    qs = urllib.parse.urlencode(params)
    sep = "&" if "?" in url else "?"
    return f"{url}{sep}{qs}"


def http_get(url: str, params: Optional[dict] = None, timeout: float = 10.0) -> tuple[Optional[dict], Optional[str], int]:
    """GET request, returns (json_data, error_msg, status_code).

    Errors are returned as (None, error_string, status_code) — never raised.
    """
    full_url = _build_url(url, params)
    try:
        req = urllib.request.Request(full_url)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.status
            data = json.loads(resp.read().decode())
            return data, None, status
    except urllib.error.HTTPError as e:
        body = e.read().decode(errors="replace")
        try:
            data = json.loads(body)
        except Exception:
            data = body
        return data, None, e.code
    except (urllib.error.URLError, OSError, socket.timeout) as e:
        return None, str(e)[:120], 0


def http_post(url: str, json_data: Optional[dict] = None,
              params: Optional[dict] = None, timeout: float = 30.0
              ) -> tuple[Optional[dict], Optional[str], int]:
    """POST request with JSON body, returns (json_data, error_msg, status_code).

    Errors are returned as (None, error_string, status_code) — never raised.
    """
    full_url = _build_url(url, params)
    body = json.dumps(json_data).encode() if json_data is not None else None
    headers = {"Content-Type": "application/json"} if body else {}
    try:
        req = urllib.request.Request(full_url, data=body, headers=headers)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.status
            data = json.loads(resp.read().decode()) if resp.read(1) else {}
            return data, None, status
    except urllib.error.HTTPError as e:
        body = e.read().decode(errors="replace")
        try:
            data = json.loads(body)
        except Exception:
            data = body
        return data, None, e.code
    except (urllib.error.URLError, OSError, socket.timeout) as e:
        return None, str(e)[:120], 0


def http_get_raw(url: str, params: Optional[dict] = None, timeout: float = 10.0
                 ) -> tuple[Optional[bytes], Optional[str], int]:
    """GET request returning raw bytes (for file downloads)."""
    full_url = _build_url(url, params)
    try:
        req = urllib.request.Request(full_url)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read(), None, resp.status
    except urllib.error.HTTPError as e:
        return None, f"HTTP {e.code}", e.code
    except (urllib.error.URLError, OSError, socket.timeout) as e:
        return None, str(e)[:120], 0
