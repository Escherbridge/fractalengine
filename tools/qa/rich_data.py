"""Execute declared rich-data operations against an owned QA run; see AGENTS.md §rich-data."""

import argparse
import base64
import copy
import datetime
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import time
import subprocess
from urllib.parse import quote, urlencode

import fe
from rich_data_report import export_report
from rich_data_scenario import NAME, ULID, ScenarioError, equal_json, epoch_ms, finite_tree, load_scenario, parse_json, require


SECRET_KEY = re.compile(r"^(authorization|password|secret|private_key|api_token|access_token|refresh_token|invite_link)$", re.I)
BEARER = re.compile(r"\bBearer\s+[^\s\"<>]+", re.I)
JWT = re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b")
LOG_RETENTION_CAPACITY = 2000


def log_retention_gap(before, after):
    valid = all(type(mark.get(key)) is int and mark[key] >= 0
                for mark in (before,after) for key in ("generation","seq"))
    return (not valid or before["generation"] != after["generation"]
            or not 0 <= after["seq"]-before["seq"] <= LOG_RETENTION_CAPACITY)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def canonical_snapshot(value):
    require(isinstance(value, dict), "journal snapshot must be an object", "invalid_response")
    snapshot = value.copy()
    for key in ("display_name", "asset_id", "tombstone"):
        snapshot.setdefault(key, None)
    return snapshot


def driver_fingerprint():
    folder = Path(__file__).parent
    return digest({name:hashlib.sha256((folder/name).read_bytes()).hexdigest()
                   for name in ("fe.py","rich_data.py","rich_data_scenario.py","rich_data_report.py")})


def scrub(value, token=""):
    if isinstance(value, dict):
        result = {}
        for key, child in value.items():
            if SECRET_KEY.fullmatch(key):
                result[key] = "[redacted]"
            elif key in ("data_base64", "files"):
                result[key] = {"omitted_encoded_bytes": len(json.dumps(child)), "sha256": digest(child)}
            else:
                result[key] = scrub(child, token)
        return result
    if isinstance(value, list):
        return [scrub(item, token) for item in value]
    if isinstance(value, str):
        value = value.replace(token, "[redacted]") if token else value
        return JWT.sub("[redacted]", BEARER.sub("Bearer [redacted]", value))
    return value


class ApiFailure(ScenarioError):
    def __init__(self, status, message):
        super().__init__("api_refused", message)
        self.status = status


class Client:
    """Use the existing loopback/token transport, with no mutation retries."""

    def __init__(self, run, verse_id, timeout):
        tokens = fe.load_tokens(run)
        require(tokens["roles"].get(verse_id) == "owner", "requested verse needs its QA owner token", "owner_token_required")
        self.token = tokens["tokens"].get(verse_id)
        require(isinstance(self.token, str) and self.token, "owner token is unavailable", "owner_token_required")
        self.run, self.timeout, self.requests = run, timeout, []
        self.deadline = time.monotonic() + timeout
        self.rpc_id = 0

    def exchange(self, method, path, body=None, read_only=False):
        require(path.startswith("/api/v1/") or path == "/mcp", "invalid fixed API path")
        for attempt in range(fe.RETRY_ATTEMPTS if read_only else 1):
            remaining = self.deadline - time.monotonic()
            require(remaining > 0, "step deadline expired", "step_timeout")
            attempt_record = {"method":method,"path":path,"request":scrub(body,self.token),"status":None}
            self.requests.append(attempt_record)
            try:
                status, _headers, raw = fe.http(self.run, method, path, self.token, body, timeout=min(remaining, 20))
            except fe.Refused as error:
                raise ScenarioError(error.code, error.message) from error
            except fe.NotRunning as error:
                code = "read_transport_failed" if read_only else "write_outcome_unknown"
                raise ScenarioError(code, "transport failed; read back before retrying a mutation") from error
            attempt_record.update(status=status,response_bytes=len(raw),response_sha256=hashlib.sha256(raw).hexdigest())
            require(len(raw) <= 8 * 1024 * 1024, "API response exceeds 8 MiB", "invalid_response")
            try:
                payload = parse_json(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError) as error:
                raise ScenarioError("invalid_response", "API did not return bounded JSON") from error
            attempt_record.update(status=status,response=scrub(payload,self.token))
            if read_only and status == 429 and attempt + 1 < fe.RETRY_ATTEMPTS:
                time.sleep(min(0.25, max(0, self.deadline-time.monotonic())))
                continue
            if not 200 <= status < 300:
                if not read_only and status >= 500:
                    raise ScenarioError("write_outcome_unknown", "server failed during a write; inspect stored state before retrying")
                raise ApiFailure(status, str(payload.get("error", "HTTP request refused")) if isinstance(payload, dict) else "HTTP request refused")
            return payload
        raise ScenarioError("read_rate_limited", "read rate budget did not recover within the step deadline")

    def rest(self, method, path, body=None, read_only=False):
        payload = self.exchange(method, path, body, read_only)
        require(isinstance(payload, dict) and type(payload.get("ok")) is bool, "missing API envelope", "invalid_response")
        if not payload["ok"]:
            message = str(payload.get("error", "operation refused"))
            if not read_only and any(word in message.lower() for word in ("not confirmed", "timed out", "timeout")):
                raise ScenarioError("write_outcome_unknown", "write confirmation unavailable; inspect stored state before retrying")
            raise ApiFailure(200, message)
        require("data" in payload, "missing API data", "invalid_response")
        return payload["data"]

    def rpc(self, method, params, read_only=True):
        self.rpc_id += 1
        payload = self.exchange("POST", "/mcp", {"jsonrpc":"2.0", "id":self.rpc_id, "method":method, "params":params}, read_only)
        require(isinstance(payload, dict) and payload.get("id") == self.rpc_id, "invalid JSON-RPC response", "invalid_response")
        if payload.get("error"):
            raise ApiFailure(200, str(payload["error"].get("message", "MCP refused")))
        result = payload.get("result")
        require(isinstance(result, dict), "missing JSON-RPC result", "invalid_response")
        if result.get("isError"):
            raise ApiFailure(200, "MCP tool refused")
        return result

    def tool(self, name, arguments, read_only=True):
        result = self.rpc("tools/call", {"name":name, "arguments":arguments}, read_only)
        blocks = result.get("content")
        require(isinstance(blocks, list) and len(blocks) == 1 and blocks[0].get("type") == "text", "unexpected MCP content", "invalid_response")
        return parse_json(blocks[0]["text"])

    def query(self, sql, variables):
        result = self.rest("POST", "/api/v1/query", {"sql":sql, "vars":variables}, read_only=True)
        require(isinstance(result, dict) and isinstance(result.get("data"), list), "invalid query rows", "invalid_response")
        return result["data"]

    def download(self, path, max_bytes=2*1024*1024):
        require(re.fullmatch(r"/api/v1/assets/by-id/[0-7][0-9A-HJKMNP-TV-Z]{25}",path), "invalid asset download path")
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, "step deadline expired", "step_timeout")
        attempt = {"method":"GET","path":path,"status":None}
        self.requests.append(attempt)
        try:
            status, _headers, raw = fe.http(self.run, "GET", path, self.token, timeout=min(remaining,20))
        except fe.Refused as error:
            raise ScenarioError(error.code, error.message) from error
        except fe.NotRunning as error:
            raise ScenarioError("read_transport_failed", "provenance asset read failed") from error
        attempt.update(status=status,response_bytes=len(raw),response_sha256=hashlib.sha256(raw).hexdigest())
        require(status == 200 and len(raw) <= max_bytes, "asset read was not confirmed within its byte bound", "invalid_response")
        return raw

    def logs(self, since=None):
        path = "/api/v1/debug/logs?" + urlencode({"level":"warn", **({"since":since} if since is not None else {})})
        try:
            payload = self.exchange("GET", path, read_only=True)
            if isinstance(payload, dict) and payload.get("ok") is True:
                payload = payload["data"]
            if not isinstance(payload, dict) or not isinstance(payload.get("entries"), list):
                return {"unavailable_reason":"debug log projection unavailable"}, []
            if not all(type(payload.get(key)) is int and payload[key] >= 0 for key in ("generation","latest_seq")):
                return {"unavailable_reason":"debug log sequence bounds unavailable"}, []
            return {"generation":payload["generation"],"seq":payload["latest_seq"],
                    "latest_seq":payload["latest_seq"]}, payload["entries"]
        except ApiFailure:
            return {"unavailable_reason":"debug log projection refused or unavailable"}, []
        except ScenarioError as error:
            if error.code == "redirect_refused":
                raise
            return {"unavailable_reason":"debug log projection unavailable: " + error.code}, []


class Evidence:
    """Keep scrubbed receipts in this pass and owned run, without overwriting an earlier execution."""

    def __init__(self, root, pass_id, run_id, scenario_id, token, verify=False):
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", pass_id), "invalid pass ID")
        pass_dir = root / "qa" / "passes" / pass_id
        require(pass_dir.resolve() == pass_dir.absolute(), "pass directory must not be redirected")
        folder = pass_dir / "rich-data" / run_id / scenario_id
        if verify:
            for number in range(1, 65):
                candidate = folder / ("restart-verification-%02d" % number)
                if not candidate.exists():
                    folder = candidate
                    break
            else:
                raise ScenarioError("evidence_exists", "restart evidence slots are exhausted")
        require(folder.resolve().is_relative_to(pass_dir.resolve()) and folder.resolve() == folder.absolute(), "evidence path is redirected")
        require(not folder.exists(), "evidence already exists; do not repeat non-idempotent writes", "evidence_exists")
        folder.mkdir(parents=True)
        self.root, self.folder, self.token = root, folder, token
        self.index = []

    def write(self, name, value, kind="api"):
        path = self.folder / (name + ".json")
        with path.open("x", encoding="utf-8") as handle:
            json.dump(scrub(value, self.token), handle, ensure_ascii=False, indent=2, allow_nan=False)
            handle.write("\n")
        entry = {"path":str(path.relative_to(self.root)), "sha256":hashlib.sha256(path.read_bytes()).hexdigest(), "kind":kind}
        self.index.append(entry)
        return entry

    def index_file(self, name, kind="tool"):
        path = self.folder / name
        entry = {"path":str(path.relative_to(self.root)),"sha256":hashlib.sha256(path.read_bytes()).hexdigest(),"kind":kind}
        self.index.append(entry)
        return entry


class Driver:
    def __init__(self, spec, run, pass_id, verse_id, petal_id, verify=False):
        self.spec, self.run, self.pass_id = spec, run, pass_id
        self.run_dir = Path(run["run_dir"]).resolve()
        require(self.run_dir.parent.name == "run" and self.run_dir.parent.parent.name == ".qa", "run must be inside an isolated .qa/run root")
        self.root, self.run_id = self.run_dir.parents[2], self.run_dir.name
        token_path = Path(run.get("tokens_path", self.run_dir / "tokens.json")).resolve()
        require(token_path.is_relative_to(self.run_dir), "QA tokens must remain inside the owned run")
        require(run.get("stage_matches_source") is True and isinstance(run.get("stage_manifest"), dict), "current-source stage provenance is required")
        manifest = run["stage_manifest"]
        require(re.fullmatch(r"[0-9a-fA-F]{40}", manifest.get("source_commit", ""))
                and re.fullmatch(r"[0-9a-fA-F]{64}", manifest.get("source_fingerprint", "")), "invalid stage provenance")
        self.provenance = {"pass_id":pass_id, "run_id":self.run_id,
                           "stage_commit":manifest["source_commit"], "stage_fingerprint":manifest["source_fingerprint"],
                           "scenario_sha256":digest(spec), "verse_id":verse_id, "petal_id":petal_id,
                           "driver_sha256":driver_fingerprint(),
                           "process_identity":run.get("process_identity"), "api_base":run["api_base"]}
        self.client = Client(run, verse_id, spec["step_timeout_seconds"])
        self.evidence = Evidence(self.root, pass_id, self.run_id, spec["id"], self.client.token, verify)
        self.verse_id, self.petal_id = verse_id, petal_id
        self.scope = None
        self.assets, self.asset_hashes, self.objects, self.states, self.versions, self.schemas = {}, {}, {}, {}, {}, {}
        self.trajectories, self.checkpoints, self.queries, self.history_views, self.derivations = {}, {}, {}, {}, []
        self.receipts, self.assertions, self.log_review = [], [], False
        self.derivation_format = None
        self.started_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
        self.coverage_records = []
        self.catalog = parse_json((self.root/"qa/feature-checkpoints.json").read_text(encoding="utf-8-sig"))
        families = {family["id"]:family for family in self.catalog["families"]}
        self.catalog_checkpoints = {item["id"]:{**families[item["family"]],**item} for item in self.catalog["checkpoints"]}
        for checkpoint in (item for group in spec.get("coverage",{}).values() for item in group):
            require(checkpoint in self.catalog_checkpoints and self.catalog_checkpoints[checkpoint]["reachability"] == "A",
                    "driver coverage mapping needs a known API checkpoint: " + checkpoint)

    def check(self, name, actual, expected, subset=False):
        passed = equal_json(actual, expected, subset=subset)
        self.assertions.append({"name":name, "actual":actual, "expected":expected, "passed":passed})
        require(passed, name + " did not match the independent expectation", "assertion_failed")

    def preflight(self, fresh=True):
        tree = self.client.rest("GET", "/api/v1/hierarchy", read_only=True)
        require(isinstance(tree, list), "invalid hierarchy", "invalid_response")
        matches = [(verse, fractal, petal) for verse in tree if verse.get("id") == self.verse_id
                   for fractal in verse.get("fractals", []) for petal in fractal.get("petals", []) if petal.get("id") == self.petal_id]
        require(len(matches) == 1, "requested petal is not in the requested owned verse", "scope_mismatch")
        verse, fractal, _petal = matches[0]
        self.scope = "VERSE#%s-FRACTAL#%s-PETAL#%s" % (verse["id"], fractal["id"], self.petal_id)
        self.check("requested petal ancestry", {"verse_id":verse["id"],"petal_id":_petal["id"]},
                   {"verse_id":self.verse_id,"petal_id":self.petal_id})
        tools = self.client.rpc("tools/list", {}).get("tools")
        require(isinstance(tools, list), "invalid MCP tool inventory", "invalid_response")
        required = {"upload_asset", "get_node_history", "read_node_at_version", "commit_node_mutation", "get_field_schema", "set_field_schema", "ingest_device_payload"}
        require(required.issubset({tool.get("name") for tool in tools}),
                "required committed-object/schema/history APIs are not available; no legacy fallback", "required_api_unavailable")
        self.check("required real API tool inventory", sorted(required & {tool.get("name") for tool in tools}), sorted(required))
        if fresh:
            rows = self.client.query("SELECT node_id FROM node WHERE petal_id = $p AND properties.qa_fixture = $fixture AND tombstone = NONE",
                                     {"p":self.petal_id, "fixture":self.spec["id"]})
            require(not rows, "this scenario already has objects in the petal; use a fresh scope or read-only restart verification", "scenario_not_fresh")
            definitions = self.client.rest("GET", "/api/v1/field-defs/"+quote(self.scope,safe=""), read_only=True)
            require(isinstance(definitions, list), "invalid field definition list", "invalid_response")
            keys = {schema["key"] for schema in self.spec["schemas"].values()}
            require(not any(definition.get("key") in keys for definition in definitions), "requested schema keys already exist in this petal", "scenario_not_fresh")
        if any(step["op"] == "query_metrics" and step["mode"] == "distributed" for step in self.spec["steps"]):
            result = self.client.rest("POST", "/api/v1/query", {"distributed":{"kind":"all_readings","petal_id":self.petal_id}}, read_only=True)
            require(isinstance(result,dict) and isinstance(result.get("distributed"),dict), "distributed query transport is unavailable", "required_api_unavailable")
            for field in ("missing_shards","missing_hosts","incomplete_shards","missing_holders","rejected_answerers","unverified_answerers"):
                self.check("preflight distributed " + field,result["distributed"].get(field),[])
            self.check("preflight distributed truncation",result["distributed"].get("truncated"),False)

    def node_path(self, name, suffix=""):
        require(name in self.objects, "logical object has no returned ID")
        return "/api/v1/nodes/" + self.objects[name] + suffix

    def coverage(self, value, node_id):
        require(isinstance(value, dict) and value.get("node_id") == node_id
                and type(value.get("journal_version")) is int and value["journal_version"] == 1
                and value.get("order") == "per_object_commit_version"
                and value.get("event_time_ordering") is False and value.get("current_state_verified") is True,
                "ordinary journal coverage is incomplete or unsupported", "history_coverage_unavailable")
        require(type(value.get("coverage_start_version")) is int and value["coverage_start_version"] == 1,
                "new object lacks its complete v1 baseline", "history_coverage_unavailable")
        require(type(value.get("current_version")) is int and value["current_version"] >= 1,
                "invalid current commit version", "invalid_response")

    def state_at(self, name, version):
        value = self.client.rest("GET", self.node_path(name, "/state?") + urlencode({"at_version":version}), read_only=True)
        self.coverage(value, self.objects[name])
        if name in self.versions:
            self.check(name + " current version during history read", value.get("current_version"), self.versions[name])
        require(value.get("availability") == "available" and value.get("at_version") == version
                and isinstance(value.get("state"), dict), "requested version is unavailable", "history_coverage_unavailable")
        return value

    def current(self, name, expected_state, expected_version):
        props = self.client.rest("GET", self.node_path(name, "/properties"), read_only=True)
        transform = self.client.rest("GET", self.node_path(name, "/transform"), read_only=True)
        page = self.client.rest("GET", self.node_path(name, "/history?") + urlencode({"after_version":expected_version-1,"limit":1}), read_only=True)
        self.coverage(page, self.objects[name])
        self.check(name + " committed version", page.get("current_version"), expected_version)
        self.check(name + " properties identity", props.get("node_id"), self.objects[name])
        self.check(name + " persisted properties", props.get("properties"), expected_state["properties"])
        require(isinstance(page.get("entries"), list) and len(page["entries"]) == 1,
                "committed version is missing from its journal page", "history_coverage_unavailable")
        self.check(name + " journal version", page["entries"][0].get("row_version"), expected_version)
        for field in ("position", "rotation", "scale"):
            self.check(name + " persisted " + field, transform.get(field), expected_state[field])
        return page

    def accept_commit(self, name, receipt, expected_version, expected_state):
        require(isinstance(receipt, dict) and receipt.get("persist") == "committed"
                and isinstance(receipt.get("state"), dict), "API did not confirm a committed object mutation", "commit_not_confirmed")
        node_id = receipt.get("node_id")
        require(isinstance(node_id, str) and ULID.fullmatch(node_id), "invalid returned object ID", "invalid_response")
        if name in self.objects:
            self.check(name + " stable identity", node_id, self.objects[name])
        else:
            require(node_id not in self.objects.values(), "two logical objects received the same ID", "invalid_response")
            self.objects[name] = node_id
        self.check(name + " receipt version", receipt.get("history_version"), expected_version)
        self.check(name + " snapshot identity", receipt["state"].get("node_id"), node_id)
        self.check(name + " snapshot petal", receipt["state"].get("petal_id"), self.petal_id)
        self.check(name + " committed intent", receipt["state"], expected_state, subset=True)
        self.check(name + " exact property map", receipt["state"].get("properties"), expected_state["properties"])
        self.current(name, receipt["state"], expected_version)
        self.states[name], self.versions[name] = receipt["state"], expected_version
        self.trajectories.setdefault(name, []).append({"version":expected_version,"position":receipt["state"]["position"]})
        return receipt

    def upload_asset(self, step):
        relative = Path(step["path"])
        require(not relative.is_absolute() and not relative.drive and ".." not in relative.parts, "asset must be a run-relative path")
        path = (self.run_dir / relative).resolve()
        require(path.is_relative_to(self.run_dir) and path.is_file(), "asset is not inside the owned run")
        require(path.stat().st_size <= 11*1024*1024, "MCP GLB exceeds 11 MiB")
        raw = path.read_bytes()
        require(raw[:4] == b"glTF", "asset is not a GLB")
        value = self.client.tool("upload_asset", {"petal_id":self.petal_id, "name":path.name,
                                  "data_base64":base64.b64encode(raw).decode("ascii")}, read_only=False)
        require(isinstance(value, dict) and isinstance(value.get("asset_id"), str) and ULID.fullmatch(value["asset_id"]), "asset upload did not return an ID", "invalid_response")
        self.assets[step["asset"]] = value["asset_id"]
        self.asset_hashes[step["asset"]] = hashlib.sha256(raw).hexdigest()
        self.check("uploaded asset byte count", value.get("size_bytes"), len(raw))
        self.check("uploaded asset byte hash", hashlib.sha256(self.client.download(
            "/api/v1/assets/by-id/"+value["asset_id"],max_bytes=11*1024*1024)).hexdigest(), hashlib.sha256(raw).hexdigest())
        return {**value,"source_sha256":hashlib.sha256(raw).hexdigest()}

    def install_schema(self, step):
        schema = self.spec["schemas"][step["schema"]]
        key = schema["key"]
        if key not in self.schemas:
            listed = self.client.rest("GET", "/api/v1/field-defs/" + quote(self.scope, safe=""), read_only=True)
            require(isinstance(listed, list) and not any(item.get("key") == key for item in listed), "schema key already exists in this scope", "scenario_not_fresh")
            created = self.client.rest("POST", "/api/v1/field-defs", {"scope":self.scope,"entity_type":"node","key":key,"value_type":schema["value_type"]})
            require(isinstance(created, dict) and isinstance(created.get("field_def_id"), str)
                    and ULID.fullmatch(created["field_def_id"]), "schema creation did not return an ID", "invalid_response")
            self.schemas[key] = {"field_def_id":created["field_def_id"]}
        schema_id = self.schemas[key]["field_def_id"]
        path = "/api/v1/field-defs/by-id/" + schema_id + "/schema"
        self.client.rest("PUT", path, schema["descriptor"])
        stored = self.client.rest("GET", path, read_only=True)
        self.check(key + " schema echo", stored.get("enforced_schema"), schema["descriptor"])
        parity = self.client.tool("get_field_schema", {"field_def_id":schema_id})
        self.check(key + " MCP schema", parity.get("enforced_schema"), stored["enforced_schema"])
        self.schemas[key] = {"field_def_id":schema_id,"descriptor":schema["descriptor"]}
        for name in self.objects:
            self.current(name, self.states[name], self.versions[name])
        return stored

    def create_object(self, step):
        name = step["object"]
        body = copy.deepcopy(self.spec["objects"][name])
        if "asset" in step:
            body["asset_id"] = self.assets[step["asset"]]
        value = self.client.rest("POST", "/api/v1/petals/" + self.petal_id + "/objects", body)
        receipt = self.accept_commit(name, value, step["expect_version"], {key:body[key] for key in ("position","rotation","scale","properties")})
        if "asset_id" in body:
            self.check(name + " committed asset binding", receipt["state"].get("asset_id"), body["asset_id"])
        return receipt

    def mutate(self, step):
        name, op = step["object"], step["op"]
        expected = copy.deepcopy(self.states[name])
        if op == "set_property":
            body = {"op":"property_set","key":step["key"],"value":step["value"]}
            expected["properties"][step["key"]] = step["value"]
        elif op == "delete_property":
            require(step["key"] in expected["properties"], "delete case must target an existing outer property")
            body = {"op":"property_deleted","key":step["key"]}
            expected["properties"].pop(step["key"], None)
        else:
            body = {"op":"full_transform", **{key:step[key] for key in ("position","rotation","scale")}}
            expected.update({key:step[key] for key in ("position","rotation","scale")})
        value = self.client.rest("POST", self.node_path(name, "/mutations"), body)
        expected_fields = {key:expected[key] for key in ("position","rotation","scale","properties")}
        return self.accept_commit(name, value, step["expect_version"], expected_fields)

    def reject_property(self, step):
        name = step["object"]
        before = copy.deepcopy(self.states[name])
        try:
            self.client.rest("POST", self.node_path(name, "/mutations"), {"op":"property_set","key":step["key"],"value":step["value"]})
        except ApiFailure as error:
            require(step.get("reason_contains", "") in str(error), "unexpected property refusal", "assertion_failed")
            self.current(name, before, self.versions[name])
            return {"refused":True,"state_and_history_unchanged":True,"message":str(error)}
        raise ScenarioError("assertion_failed", "invalid property was accepted")

    def reject_schema(self, step):
        schema = self.spec["schemas"][step["schema"]]
        path = "/api/v1/field-defs/by-id/" + self.schemas[schema["key"]]["field_def_id"] + "/schema"
        before = self.client.rest("GET", path, read_only=True)
        try:
            self.client.rest("PUT", path, schema["descriptor"])
        except ApiFailure as error:
            require(step.get("reason_contains", "") in str(error), "unexpected schema refusal", "assertion_failed")
            self.check("rejected schema unchanged", self.client.rest("GET", path, read_only=True), before)
            for name in self.objects:
                self.current(name, self.states[name], self.versions[name])
            return {"refused":True,"schema_state_history_unchanged":True,"message":str(error)}
        raise ScenarioError("assertion_failed", "incompatible schema was accepted")

    def reading_rows(self, metric, start_ms, end_ms, names):
        return self.client.query(
            "SELECT reading_id, node_id, metric, value, units, recorded_at_ms, payload_id FROM iot_reading "
            "WHERE petal_id = $p AND metric = $metric AND recorded_at_ms >= $start AND recorded_at_ms < $end "
            "AND node_id IN $nodes ORDER BY recorded_at_ms, reading_id",
            {"p":self.petal_id,"metric":metric,"start":start_ms,"end":end_ms,"nodes":[self.objects[name] for name in names]})

    def write_readings(self, metric, units, timestamp, values, payload_ids=None):
        time_ms = epoch_ms(timestamp)
        before = self.reading_rows(metric, time_ms, time_ms+1, values)
        require(not before, "readings already exist at this scenario event; no automatic append retry", "reading_already_exists")
        readings = [{"node_id":self.objects[name],"metric":metric,"value":value,"units":units,"recorded_at":timestamp,
                     **({"payload_id":payload_ids[name]} if payload_ids else {})} for name, value in values.items()]
        response = self.client.exchange("POST", "/api/v1/petals/" + self.petal_id + "/iot/readings", {"readings":readings})
        require(isinstance(response, dict) and response.get("ok") is True, "measurement append was refused", "append_not_confirmed")
        self.check("accepted reading count", response.get("accepted"), len(readings))
        while True:
            rows = self.reading_rows(metric, time_ms, time_ms+1, values)
            require(len(rows) <= len(readings), "unexpected duplicate readings after append", "reading_duplicate_detected")
            if len(rows) == len(readings):
                ids = [row.get("reading_id") for row in rows]
                require(all(isinstance(item,str) and ULID.fullmatch(item) for item in ids) and len(set(ids)) == len(ids),
                        "stored readings lack unique IDs", "invalid_response")
                by_node = {row.get("node_id"):row for row in rows}
                require(len(by_node) == len(readings), "reading anchors are duplicated", "reading_duplicate_detected")
                for name, expected in values.items():
                    row = by_node.get(self.objects[name], {})
                    self.check(name + " appended metric", row.get("metric"), metric)
                    self.check(name + " appended value", row.get("value"), expected)
                    self.check(name + " appended units", row.get("units"), units)
                    self.check(name + " sensor event time", row.get("recorded_at_ms"), time_ms)
                    if payload_ids:
                        self.check(name + " payload provenance", row.get("payload_id"), payload_ids[name])
                return rows
            require(time.monotonic() < self.client.deadline, "accepted measurements did not appear in stored readback", "readback_timeout")
            time.sleep(min(0.15, max(0, self.client.deadline-time.monotonic())))

    def append_readings(self, step):
        return self.write_readings(step["metric"], step["units"], step["recorded_at"], step["values"])

    def derive_metric(self, step):
        names = step["calibration_versions"]
        rows = self.reading_rows(step["source_metric"], step["start_ms"], step["end_ms"], names)
        require(rows and len(rows) <= 128, "derivation needs 1..128 actual stored rows")
        if self.derivation_format is None:
            self.derivation_format = "qa.fractalengine/" + self.spec["id"].lower() + "@1"
            self.client.tool("register_device_format", {"verse_id":self.verse_id,"format_id":self.derivation_format,
                "mime":"application/json", "description":"Explicit QA affine derivation provenance; opaque bytes, not an automatic rule engine",
                "schema":{"type":"object","required":["source_metric","history_version","raw","gain","bias","derived"]}}, read_only=False)
        reverse = {node_id:name for name, node_id in self.objects.items()}
        grouped = {}
        provenance = []
        for row in rows:
            name = reverse.get(row.get("node_id"))
            require(name in names, "unexpected derivation anchor", "invalid_response")
            at = self.state_at(name, names[name])
            self.check(name + " calibration snapshot node", at["state"].get("node_id"),self.objects[name])
            self.check(name + " calibration snapshot petal", at["state"].get("petal_id"),self.petal_id)
            calibration = at["state"].get("properties", {}).get("calibration")
            require(isinstance(calibration, dict) and all(type(calibration.get(key)) in (int,float) for key in ("gain","bias")),
                    "historical calibration is not numeric", "invalid_response")
            page = self.client.rest("GET", self.node_path(name, "/history?") + urlencode({"after_version":names[name]-1,"limit":1}), read_only=True)
            self.coverage(page, self.objects[name])
            require(isinstance(page.get("entries"), list) and len(page["entries"]) == 1
                    and page["entries"][0].get("row_version") == names[name], "calibration fact is missing", "history_coverage_unavailable")
            fact = page["entries"][0]
            self.check(name + " calibration fact node",fact.get("node_id"),self.objects[name])
            self.check(name + " calibration fact commit",fact.get("row_version"),names[name])
            fact_payload = fact.get("payload",{})
            self.check(name + " calibration fact scope",fact_payload.get("scope"),self.scope)
            self.check(name + " calibration fact state",
                       canonical_snapshot(fact_payload.get("after_state")), canonical_snapshot(at["state"]))
            schema_versions = fact_payload.get("schema_versions")
            require(isinstance(schema_versions, dict), "calibration schema provenance unavailable", "history_coverage_unavailable")
            require("calibration" in self.schemas,"calibration field definition is unavailable", "history_coverage_unavailable")
            schema_id = self.schemas["calibration"]["field_def_id"]
            expected_revision = step["calibration_schema_versions"][name]
            actual_revision = schema_versions.get(schema_id)
            require(type(actual_revision) is int,"calibration schema revision is not an integer", "history_coverage_unavailable")
            self.check(name + " independently expected calibration schema revision",actual_revision,expected_revision)
            raw_value, time_ms = row.get("value"), row.get("recorded_at_ms")
            require(type(raw_value) in (int,float) and type(time_ms) is int, "invalid raw reading", "invalid_response")
            derived = calibration["gain"] * raw_value + calibration["bias"]
            require(math.isfinite(derived) and abs(derived) <= 1e15, "affine result exceeds the QA driver numeric bound")
            timestamp = (datetime.datetime(1970,1,1,tzinfo=datetime.timezone.utc)+datetime.timedelta(milliseconds=time_ms)).isoformat().replace("+00:00","Z")
            record = {"source_metric":step["source_metric"],"target_metric":step["target_metric"],"node_id":self.objects[name],
                      "history_version":names[name],"schema_versions":schema_versions,
                      "expected_calibration_schema_revision":expected_revision,"raw":raw_value,
                      "gain":calibration["gain"],"bias":calibration["bias"],"formula":"gain * raw + bias",
                      "derived":derived,"recorded_at_ms":time_ms,"source_reading_id":row.get("reading_id")}
            payload_bytes = json.dumps(record, sort_keys=True, allow_nan=False).encode()
            encoded = base64.b64encode(payload_bytes).decode("ascii")
            payload = self.client.tool("ingest_device_payload", {"petal_id":self.petal_id,"node_id":self.objects[name],
                "format_id":self.derivation_format,"recorded_at":timestamp,"files":[encoded]}, read_only=False)
            require(isinstance(payload, dict) and isinstance(payload.get("payload_id"), str) and ULID.fullmatch(payload["payload_id"]),
                    "derivation payload was not confirmed", "invalid_response")
            require(isinstance(payload.get("asset_ids"), list) and len(payload["asset_ids"]) == 1
                    and isinstance(payload["asset_ids"][0], str) and ULID.fullmatch(payload["asset_ids"][0]),
                    "provenance asset ID missing", "invalid_response")
            payload_sha = hashlib.sha256(payload_bytes).hexdigest()
            self.check(name + " stored provenance bytes", hashlib.sha256(self.client.download(
                "/api/v1/assets/by-id/" + payload["asset_ids"][0])).hexdigest(), payload_sha)
            require(name not in grouped.setdefault(timestamp, {}), "multiple source rows for one event/anchor")
            grouped[timestamp][name] = (derived, payload["payload_id"])
            provenance.append({**record,"payload_id":payload["payload_id"],"asset_ids":payload["asset_ids"],"payload_sha256":payload_sha})
        persisted = []
        for timestamp, values in grouped.items():
            persisted.extend(self.write_readings(step["target_metric"],step["units"],timestamp,
                {name:value[0] for name,value in values.items()}, {name:value[1] for name,value in values.items()}))
        self.derivations.extend(provenance)
        return {"derivation_is_explicit_driver_action":True,"provenance":provenance,"stored_readings":persisted}

    def complete_distribution(self, metadata):
        require(isinstance(metadata, dict), "distributed honesty metadata missing", "incomplete_query")
        for field in ("missing_shards","missing_hosts","incomplete_shards","missing_holders","rejected_answerers","unverified_answerers"):
            self.check("complete distributed " + field, metadata.get(field), [])
        self.check("distributed truncation", metadata.get("truncated"), False)
        require(isinstance(metadata.get("covered_shards"), list) and metadata["covered_shards"]
                and isinstance(metadata.get("answered_hosts"), list) and metadata["answered_hosts"],
                "no actual covered shards/hosts", "incomplete_query")

    def query_metrics(self, step):
        oracle = self.spec["oracles"][step["oracle"]]
        if step["mode"] == "distributed":
            kind = {"rows":"readings_in_window","aggregate":"window_aggregate","latest":"latest_per_anchor"}[step["kind"]]
            query = {"kind":kind,"petal_id":self.petal_id,"metric":step["metric"]}
            if kind != "latest_per_anchor":
                query.update(start_ms=step["start_ms"],end_ms=step["end_ms"])
            result = self.client.rest("POST", "/api/v1/query", {"distributed":query}, read_only=True)
            require(isinstance(result, dict) and isinstance(result.get("data"), list), "invalid distributed rows", "invalid_response")
            rows, metadata = result["data"], result.get("distributed")
            self.complete_distribution(metadata)
            if step.get("mcp_parity"):
                parity = self.client.tool("query_timeseries", query)
                require(isinstance(parity, dict) and parity.get("error") is None, "MCP timeseries refused", "invalid_response")
                self.complete_distribution(parity.get("meta"))
                self.check("MCP timeseries rows", sorted(parity["rows"],key=lambda row:json.dumps(row,sort_keys=True)),
                           sorted(rows,key=lambda row:json.dumps(row,sort_keys=True)))
        else:
            metadata = None
            variables = {"p":self.petal_id,"metric":step["metric"],"start":step["start_ms"],"end":step["end_ms"],
                         "nodes":[self.objects[name] for name in oracle]}
            predicate = "FROM iot_reading WHERE petal_id = $p AND metric = $metric AND recorded_at_ms >= $start AND recorded_at_ms < $end AND node_id IN $nodes"
            if step["kind"] == "aggregate":
                rows = self.client.query("SELECT node_id, metric, count() AS sample_count, math::sum(value) AS sum_value, math::mean(value) AS avg_value, math::min(value) AS min_value, math::max(value) AS max_value " + predicate + " GROUP BY node_id, metric", variables)
            elif step["kind"] == "latest":
                rows = self.client.query("SELECT node_id, metric, value, recorded_at_ms " + predicate +
                    " AND recorded_at_ms = math::max((SELECT VALUE recorded_at_ms FROM iot_reading WHERE node_id = $parent.node_id AND metric = $parent.metric))", variables)
            else:
                rows = self.reading_rows(step["metric"],step["start_ms"],step["end_ms"],oracle)
        reverse = {node_id:name for name,node_id in self.objects.items()}
        groups = {name:[] for name in oracle}
        for row in rows:
            name = reverse.get(row.get("node_id"))
            require(name in groups, "query returned an unexpected object", "assertion_failed")
            self.check(name + " queried metric", row.get("metric"), step["metric"])
            groups[name].append(row)
        for name, expected in oracle.items():
            if step["kind"] == "aggregate":
                self.check(name + " aggregate row count", len(groups[name]), 1)
                row = groups[name][0]
                stats = {"count":row.get("sample_count"),"min":row.get("min_value"),"max":row.get("max_value"),"mean":row.get("avg_value")}
                self.check(name + " per-anchor aggregate", stats, {key:expected["stats"][key] for key in stats})
                if "sum_value" in row:
                    self.check(name + " per-anchor sum", row["sum_value"], expected["stats"]["sum"])
            else:
                ordered = sorted(groups[name],key=lambda row:(row["recorded_at_ms"],row.get("reading_id","")))
                values = [row["value"] for row in ordered]
                self.check(name + " chronological sensor times", [row["recorded_at_ms"] for row in ordered], expected["times_ms"])
                self.check(name + " independently expected values", values, expected["values"])
                stats = {"count":len(values),"min":min(values),"max":max(values),"mean":math.fsum(values)/len(values),"sum":math.fsum(values)}
                self.check(name + " per-anchor statistics", stats, expected["stats"])
        value = {"kind":step["kind"],"metric":step["metric"],"mode":step["mode"],"rows":rows,"distributed":metadata}
        self.queries[step["id"]] = value
        return value

    def checkpoint(self, step):
        rows = self.client.query("SELECT node_id FROM node WHERE petal_id = $p AND properties.qa_fixture = $fixture AND tombstone = NONE",
                                 {"p":self.petal_id,"fixture":self.spec["id"]})
        self.check("created object set at checkpoint", sorted(row.get("node_id") for row in rows), sorted(self.objects.values()))
        snapshots = {}
        for name, expected in step["expect"].items():
            self.current(name, self.states[name], self.versions[name])
            value = self.state_at(name, self.versions[name])
            self.check(name + " independent checkpoint", value["state"], expected, subset=True)
            self.check(name + " replay matches committed snapshot", value["state"], self.states[name])
            snapshots[name] = {"node_id":self.objects[name],"history_version":self.versions[name],
                               "state":value["state"],"state_sha256":digest(value["state"])}
        self.checkpoints[step["id"]] = snapshots
        return {"cut_semantics":"explicit per-object committed-version map; not global time","objects":snapshots}

    def historical_state(self, step):
        name = step["object"]
        value = self.state_at(name, step["version"])
        self.check(name + " earlier independent state", value["state"], step["expect"], subset=True)
        if step.get("mcp_parity"):
            parity = self.client.tool("read_node_at_version", {"node_id":self.objects[name],"at_version":step["version"]})
            self.coverage(parity, self.objects[name])
            self.check("MCP historical state", parity.get("state"), value["state"])
        self.current(name, self.states[name], self.versions[name])
        return value

    def history(self, step):
        name, cursor, entries = step["object"], step["after_version"], []
        for _ in range(64):
            args = {"after_version":cursor,"limit":step["limit"]}
            page = self.client.rest("GET", self.node_path(name, "/history?")+urlencode(args), read_only=True)
            self.coverage(page, self.objects[name])
            self.check(name + " history current version", page.get("current_version"), self.versions[name])
            require(isinstance(page.get("entries"), list) and len(page["entries"]) <= step["limit"], "invalid history page", "invalid_response")
            versions = [entry.get("row_version") for entry in page["entries"]]
            require(all(type(v) is int and v > cursor for v in versions) and versions == sorted(set(versions)), "history cursor is not monotonic", "invalid_response")
            if step.get("mcp_parity"):
                parity = self.client.tool("get_node_history", {"node_id":self.objects[name],**args})
                self.check("MCP history page", parity, page)
            entries.extend(page["entries"])
            next_cursor = page.get("next_after_version")
            if next_cursor is None:
                break
            require(versions and next_cursor == versions[-1] and next_cursor > cursor, "history cursor did not advance", "invalid_response")
            cursor = next_cursor
        else:
            raise ScenarioError("history_page_limit", "history exceeded 64 pages")
        self.check(name + " complete paginated versions", [entry["row_version"] for entry in entries], step["expect_versions"])
        self.history_views[name] = entries
        return {"entries":entries,"after_version":step["after_version"],"limit":step["limit"],"complete":True}

    def distances(self, step):
        distances = {name:math.fsum(math.dist(a["position"],b["position"]) for a,b in zip(points,points[1:]))
                     for name,points in self.trajectories.items()}
        self.check("stored three-dimensional travel", {name:distances[name] for name in step["expected_m"]}, step["expected_m"])
        return {"distance_m":distances,"basis":"stored XYZ snapshots after each committed mutation; rotation/scale adds zero travel"}

    def context(self, complete):
        return {"format":"fe-rich-data-context-v1","provenance":self.provenance,"scope":self.scope,
                "data_assertions_succeeded":complete,"log_review_required":self.log_review,"started_at":self.started_at,
                "objects":self.objects,"versions":self.versions,"states":self.states,
                "state_sha256":{name:digest(state) for name,state in self.states.items()},
                "schemas":self.schemas,"assets":self.assets,"asset_sha256":self.asset_hashes,"trajectories":self.trajectories,
                "checkpoints":self.checkpoints,"queries":self.queries,"history":self.history_views,
                "derivations":self.derivations,"derivation_format":self.derivation_format,
                "completed_step_ids":[receipt["step_id"] for receipt in self.receipts if receipt["outcome"] == "PASS"]}

    def execute_step(self, step, handler, number):
        self.client.deadline = time.monotonic() + self.spec["step_timeout_seconds"]
        self.client.requests, self.assertions = [], []
        watermark = {"unavailable_reason":"step did not obtain its log watermark"}
        receipt = {**self.provenance,"step_id":step["id"],"intent":step,"outcome":"FAIL"}
        failure = None
        try:
            watermark, _old_logs = self.client.logs()
            receipt["result"] = handler(step)
            receipt["outcome"] = "PASS"
        except (ScenarioError, fe.NotRunning) as error:
            failure = error
            receipt["failure"] = {"code":getattr(error,"code","not_running"),"message":str(error)}
        except (KeyError, TypeError, ValueError, AttributeError, IndexError, OverflowError) as error:
            failure = ScenarioError("invalid_response", "response shape or value violated the declared operation contract")
            receipt["failure"] = {"code":failure.code,"message":str(failure),"detail_type":type(error).__name__}
        try:
            after, new_logs = self.client.logs(watermark.get("seq"))
        except ScenarioError as error:
            after, new_logs = {"unavailable_reason":error.code}, []
            if failure is None:
                failure = error
                receipt["failure"] = {"code":error.code,"message":str(error)}
        if failure:
            receipt["outcome"] = "FAIL"
        generation_changed = "generation" in watermark and after.get("generation") != watermark["generation"]
        retention_gap = log_retention_gap(watermark,after)
        needs_review = bool(new_logs) or "unavailable_reason" in watermark or "unavailable_reason" in after or retention_gap
        self.log_review |= needs_review
        receipt.update(assertions=self.assertions,requests=self.client.requests,log_watermark=watermark,
                       log_end_watermark=after,new_logs=new_logs,log_generation_changed=generation_changed,
                       log_retention_gap=retention_gap,log_retention_capacity=LOG_RETENTION_CAPACITY,log_review_required=needs_review)
        artifact = self.evidence.write("%02d-%s" % (number,step["id"]), receipt, "persistence")
        self.receipts.append({"step_id":step["id"],"outcome":receipt["outcome"],"artifact":artifact,
                              "log_review_required":needs_review})
        for checkpoint in self.spec.get("coverage",{}).get(step["id"],[]):
            outcome = "FAIL" if failure else ("UNKNOWN" if needs_review else "PASS")
            artifacts = [{**artifact,"kind":"api"}]
            if self.catalog_checkpoints[checkpoint]["requires_persistence"]:
                artifacts.append({**artifact,"kind":"persistence"})
            record = {key:self.provenance[key] for key in ("pass_id","run_id","stage_commit","stage_fingerprint")}
            record.update(checkpoint=checkpoint,outcome=outcome,
                          commands=[subprocess.list2cmdline([sys.executable,*sys.argv])+" [step "+step["id"]+"]"],
                          assertions=self.assertions,artifacts=artifacts,log_watermark=watermark,
                          log_end_watermark=after,log_retention_gap=retention_gap,
                          new_logs=new_logs,reason=("API operation failed" if failure else "Log evidence needs independent review") if outcome != "PASS" else None)
            self.coverage_records.append(record)
        print(json.dumps({"step_id":step["id"],"outcome":receipt["outcome"],"artifact":artifact["path"],
                          "log_review_required":needs_review}), flush=True)
        if failure:
            raise failure
        return receipt

    def verify_restart(self, path):
        path = Path(path).resolve()
        pass_dir = self.root / "qa" / "passes" / self.pass_id
        require(path.is_relative_to(pass_dir.resolve()), "restart context must belong to this current pass")
        saved = parse_json(path.read_text(encoding="utf-8-sig"))
        require(isinstance(saved, dict) and saved.get("format") == "fe-rich-data-context-v1"
                and saved.get("data_assertions_succeeded") is True, "restart needs a completed actual execution context")
        prior = saved.get("provenance", {})
        for field in ("pass_id","run_id","verse_id","petal_id","scenario_sha256","stage_commit","stage_fingerprint","driver_sha256"):
            self.check("restart " + field, prior.get(field), self.provenance[field])
        old_identity, new_identity = prior.get("process_identity"), self.provenance.get("process_identity")
        require(isinstance(old_identity,dict) and isinstance(new_identity,dict), "restart needs both recorded process identities")
        self.check("owned app process identity refreshed", (old_identity.get("pid"),old_identity.get("start_ticks")) !=
                   (new_identity.get("pid"),new_identity.get("start_ticks")), True)
        require(set(saved["objects"]) == set(self.spec["objects"]), "restart object set is incomplete")
        self.objects, self.versions = saved["objects"], saved["versions"]
        require(all(isinstance(node_id,str) and ULID.fullmatch(node_id) for node_id in self.objects.values()), "invalid restart object ID")
        self.schemas, self.assets, self.derivation_format = saved["schemas"], saved["assets"], saved["derivation_format"]
        self.asset_hashes = saved["asset_sha256"]
        for name, asset_id in self.assets.items():
            require(isinstance(asset_id,str) and ULID.fullmatch(asset_id), "invalid saved GLB asset ID")
            raw = self.client.download("/api/v1/assets/by-id/"+asset_id,max_bytes=11*1024*1024)
            self.check("restart uploaded GLB bytes " + name, hashlib.sha256(raw).hexdigest(),self.asset_hashes[name])
        self.log_review |= saved.get("log_review_required", False)
        for name, version in self.versions.items():
            state = self.state_at(name, version)["state"]
            self.check(name + " exact restart state digest", digest(state), saved["state_sha256"][name])
            self.current(name, state, version)
            self.states[name] = state
        for key, schema in self.schemas.items():
            row = self.client.rest("GET", "/api/v1/field-defs/by-id/"+schema["field_def_id"]+"/schema", read_only=True)
            self.check(key + " restart schema", row.get("enforced_schema"), schema["descriptor"])
        self.checkpoints = saved["checkpoints"]
        for step_id, objects in self.checkpoints.items():
            for name, snapshot in objects.items():
                state = self.state_at(name, snapshot["history_version"])["state"]
                self.check(step_id + " replay digest " + name, digest(state), snapshot["state_sha256"])
        for name, points in saved["trajectories"].items():
            actual = []
            for point in points:
                state = self.state_at(name,point["version"])["state"]
                self.check(name + " stored history position", state["position"], point["position"])
                actual.append({"version":point["version"],"position":state["position"]})
            self.trajectories[name] = actual
        self.derivations = saved["derivations"]
        for item in self.derivations:
            require(len(item["asset_ids"]) == 1, "invalid saved provenance asset")
            raw = self.client.download("/api/v1/assets/by-id/"+item["asset_ids"][0])
            self.check("restart provenance bytes", hashlib.sha256(raw).hexdigest(), item["payload_sha256"])
        return {"read_only":True,"same_root":self.run_id,"replayed_checkpoints":list(self.checkpoints)}

    def run_steps(self, verify_path=None):
        # Command pattern: validated verbs map to explicit effect handlers.
        handlers = {"upload_asset":self.upload_asset,"install_schema":self.install_schema,
                    "create_object":self.create_object,"set_property":self.mutate,"delete_property":self.mutate,
                    "transform":self.mutate,"append_readings":self.append_readings,"derive_metric":self.derive_metric,
                    "query_metrics":self.query_metrics,"checkpoint":self.checkpoint,"state_at":self.historical_state,
                    "history":self.history,"reject_property":self.reject_property,"reject_schema":self.reject_schema,
                    "distances":self.distances}
        complete, failure = False, None
        try:
            self.execute_step({"id":"preflight","op":"preflight"},lambda _step:self.preflight(fresh=not verify_path),0)
            if verify_path:
                verify_step = {"id":"restart-state-and-blobs","op":"restart-verification"}
                self.execute_step(verify_step, lambda _step:self.verify_restart(verify_path), 1)
                steps = [step for step in self.spec["steps"] if step["op"] in {"query_metrics","state_at","history","distances"}]
            else:
                steps = self.spec["steps"]
            for number, step in enumerate(steps, 2 if verify_path else 1):
                self.execute_step(step, handlers[step["op"]], number)
            complete = True
        except (ScenarioError, fe.NotRunning) as error:
            failure = {"code":getattr(error,"code","not_running"),"message":scrub(str(error),self.client.token)}
        context = scrub(self.context(complete),self.client.token)
        context_artifact = self.evidence.write("context", context, "persistence")
        exports = [self.evidence.index_file(name) for name in export_report(self.evidence.folder,context)]
        summary = {**self.provenance,"scenario":self.spec["id"],"data_assertions_succeeded":complete,
                   "coverage_ready":complete and not self.log_review,"log_review_required":self.log_review,
                   "read_only_restart_verification":bool(verify_path),"failure":failure,
                   "context":context_artifact,"exports":exports,"receipts":self.receipts,
                   "limits":["API/driver evidence only; no GUI screenshot or product chart was exercised",
                             "measurement event time differs from object commit-version order",
                             "single-host complete queries do not establish replicated durability"]}
        self.evidence.write("summary", summary)
        with (self.evidence.folder/"coverage-candidates.jsonl").open("x",encoding="utf-8") as handle:
            for record in self.coverage_records:
                handle.write(json.dumps(scrub(record,self.client.token),allow_nan=False)+"\n")
        self.evidence.index_file("coverage-candidates.jsonl")
        self.evidence.write("artifact-index", self.evidence.index)
        return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--run")
    parser.add_argument("--pass-id")
    parser.add_argument("--verse-id")
    parser.add_argument("--petal-id")
    parser.add_argument("--verify-state", help="read-only same-root verification using this pass's completed context JSON")
    args = parser.parse_args(argv)
    try:
        spec = load_scenario(args.scenario)
        if args.validate_only:
            print(json.dumps({"validation_only":True,"scenario":spec["id"],"steps":len(spec["steps"]),
                              "operations_executed":0,"both_history_and_measurements_verified":False}))
            return 0
        require(all((args.run,args.pass_id,args.verse_id,args.petal_id)), "execution requires explicit --run/--pass-id/--verse-id/--petal-id")
        require(ULID.fullmatch(args.verse_id) and ULID.fullmatch(args.petal_id), "verse/petal must be actual ULIDs")
        run = fe.load_run(args.run)
        driver = Driver(spec,run,args.pass_id,args.verse_id,args.petal_id,bool(args.verify_state))
        summary = driver.run_steps(args.verify_state)
        print(json.dumps(scrub(summary,driver.client.token),indent=2))
        return 0 if summary["coverage_ready"] else 2
    except fe.NotRunning as error:
        print(json.dumps({"error":"not_running","message":str(error)}))
        return 3
    except (ScenarioError, OSError, ValueError, TypeError, KeyError, AttributeError, IndexError) as error:
        print(json.dumps({"error":getattr(error,"code","invalid_driver_input_or_response"),"message":scrub(str(error))}))
        return 2


if __name__ == "__main__":
    sys.exit(main())
