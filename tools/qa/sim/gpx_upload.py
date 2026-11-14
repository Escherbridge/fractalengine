"""Multipart GPX upload op for the simulator driver (REST door of the GPX importer)."""
import os
import uuid

from client import Outcome

SIM_DIR = os.path.dirname(os.path.abspath(__file__))


def upload_gpx(client, petal_id, file_path, name=None, timeout=300.0):
    """POST a .gpx file to /api/v1/petals/{id}/import/gpx; relative paths resolve against tools/qa/sim."""
    token, source = client.verse_token()
    if not token:
        return Outcome(status=0, ok=False, refused=True, code="no_token_for_verse", token_source=source)
    resolved = file_path if os.path.isabs(file_path) else os.path.join(SIM_DIR, file_path)
    boundary = "fe-sim-" + uuid.uuid4().hex
    with open(resolved, "rb") as handle:
        content = handle.read()
    filename = name or os.path.basename(resolved)
    head = ("--%s\r\nContent-Disposition: form-data; name=\"file\"; filename=\"%s\"\r\n"
            "Content-Type: application/gpx+xml\r\n\r\n" % (boundary, filename)).encode("utf-8")
    body = head + content + ("\r\n--%s--\r\n" % boundary).encode("utf-8")
    out = client.send("POST", "/api/v1/petals/%s/import/gpx" % petal_id, None, token, timeout, raw=body,
                      content_type="multipart/form-data; boundary=" + boundary)
    out["token_source"] = source
    return out
