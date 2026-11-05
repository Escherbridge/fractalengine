import hashlib
from pathlib import Path
import re

PATTERNS = {
    'bearer_value': r'(?i)\bbearer\s+[A-Za-z0-9_.~+/=-]{16,}',
    'jwt_value': r'\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b',
    'private_key_block': r'-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----',
    'credential_assignment': r'''(?i)["']?(?:access_token|refresh_token|api_key|password|secret_key)["']?\s*[:=]\s*["']?[A-Za-z0-9_./+~=-]{16,}''',
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def describe_file(path):
    data = path.read_bytes()
    return dict(bytes=len(data), sha256=digest(data))


def decode_text(data):
    if data.startswith((b'\xff\xfe', b'\xfe\xff')):
        text, encoding = data.decode('utf-16'), 'utf-16-bom'
    elif data.startswith(b'\xef\xbb\xbf'):
        text, encoding = data.decode('utf-8-sig'), 'utf-8-bom'
    else:
        text, encoding = data.decode('utf-8'), 'utf-8'
    assert '\x00' not in text
    return text, encoding


def scan(text):
    return {key: len(re.findall(pattern, text)) for key, pattern in PATTERNS.items()}


def describe_log(path):
    data = path.read_bytes()
    text, encoding = decode_text(data)
    clean = re.sub(r'\x1b\[[0-9;]*m', '', text)
    return dict(local_path=str(path), bytes=len(data), sha256=digest(data),
                text_encoding=encoding, lossy_decoding=False,
                line_count=len(text.splitlines()),
                levels={level: len(re.findall(r'(?m)^\d{4}-\d\d-\d\dT\S+\s+' + level + r'\b', clean)) for level in ['INFO', 'WARN', 'ERROR']},
                panic_markers=len(re.findall(r'panicked at|thread [^\n]* panicked', clean)),
                compiler_warning_diagnostics=len(re.findall(r'(?m)^warning:(?! build failed| `)', clean)),
                compiler_error_diagnostics=len(re.findall(r'(?m)^error(?:\[|:)', clean)),
                private_current_error_markers=clean.count('scene current read transaction'),
                credential_pattern_counts=scan(text), raw_log_published=False)


def copy_exact(source, target):
    data = source.read_bytes()
    target.parent.mkdir(parents=True, exist_ok=True)
    assert not target.exists(), target
    target.write_bytes(data)
    assert target.read_bytes() == data
    return data


def manifest_files(folder):
    return [dict(file=path.relative_to(folder).as_posix(), **describe_file(path))
            for path in sorted(folder.rglob('*'))
            if path.is_file() and path.name != 'packet-manifest.json']
