"""Reconcile source producers with catalog owners; see AGENTS.md section catalog census."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys


CONTROL = re.compile(r"\.(named|button_named|button|small_button|checkbox|radio_value|selectable_value|selectable_label|text_edit_singleline|text_edit_multiline|color_edit_button_\w+|drag_angle|add_enabled|menu_button|toggle_value|collapsing|link|hyperlink|hyperlink_to)\s*\(|(?<!\w)(Button|TextEdit|DragValue|Slider|ComboBox|MenuButton|RadioButton|Checkbox|CollapsingHeader)::|(?<!\w)(button|confirm_button)\s*\(")


def production(text):
    for match in reversed(list(re.finditer(r"#\[cfg\(test\)\]",text))):
        start=match.start()
        body=text.find("{",match.end())
        end_statement=text.find(";",match.end())
        if body>=0 and (end_statement<0 or body<end_statement):
            _,end=balanced(text,body,"{","}")
        elif end_statement>=0:
            end=end_statement+1
        else:
            raise ValueError("unrecognized test-only item")
        text=text[:start]+"\n"*text[start:end].count("\n")+text[end:]
    return text


def balanced(text, start, opening="(", closing=")"):
    depth, quoted, escaped = 0, False, False
    for index in range(start,len(text)):
        char = text[index]
        if quoted:
            if escaped:
                escaped=False
            elif char == "\\":
                escaped=True
            elif char == '"':
                quoted=False
        elif char == '"':
            quoted=True
        elif char == opening:
            depth+=1
        elif char == closing:
            depth-=1
            if depth==0:
                return text[start+1:index],index+1
    raise ValueError("unbalanced source delimiter")


def case_id(method, route):
    return "F116.rest-"+method.lower()+"-"+re.sub(r"[^a-z0-9]+","-",route.lower()).strip("-")


def census(root, snapshot):
    catalog=json.loads((root/"qa/feature-checkpoints.json").read_text(encoding="utf-8-sig"))
    if catalog.get("audited_source_commit") != snapshot:
        raise ValueError("requested source commit differs from the reviewed catalog snapshot")
    checkpoint_ids=[item["id"] for item in catalog["checkpoints"]]
    if len(checkpoint_ids)!=len(set(checkpoint_ids)):
        raise ValueError("duplicate catalog checkpoint ID")
    rules=json.loads((root/"tools/qa/catalog_source_rules.json").read_text(encoding="utf-8"))
    families={family["id"] for family in catalog["families"]}
    owners={}
    for family in catalog["families"]:
        for source in family["sources"]:
            owners.setdefault(source,set()).add(family["id"])
    for source,owned in rules["files"].items():
        if not set(owned).issubset(families):
            raise ValueError("unknown source family: "+source)
        owners[source]=set(owned)
    texts={}
    for directory in ("fe-ui/src","fe-api/src","fe-runtime/src","fe-plugin/src","fe-webview/src","fractalengine/src"):
        for path in sorted((root/directory).rglob("*.rs")):
            texts[path.relative_to(root).as_posix()]=production(path.read_text(encoding="utf-8"))
    actions=[]
    action_source=texts["fe-ui/src/actions/mod.rs"]
    opening=action_source.index("{",action_source.index("pub enum UiAction"))
    body,_=balanced(action_source,opening,"{","}")
    variants=re.findall(r"^    ([A-Z]\w*)\s*(?:\{|\(|,)",body,re.M)
    unknown_actions=set(rules["actions"])-set(variants)
    if unknown_actions:
        raise ValueError("unknown action ownership keys: "+", ".join(sorted(unknown_actions)))
    for variant in variants:
        refs=[]
        for source,text in texts.items():
            if not source.startswith("fe-ui/src/") or source.startswith("fe-ui/src/actions/"):
                continue
            for match in re.finditer(r"\bUiAction::"+variant+r"\b",text):
                line=text.count("\n",0,match.start())+1
                current=text.splitlines()[line-1].strip()
                if not current.startswith("//"):
                    refs.append({"source":source,"line":line,"families":sorted(owners.get(source,set()))})
        owned=set(rules["actions"].get(variant,[])) or {family for ref in refs for family in ref["families"]}
        if not owned.issubset(families):
            raise ValueError("unknown action family: "+variant)
        actions.append({"action":variant,"families":sorted(owned),"render_or_gesture_references":refs,
                        "note":rules["action_notes"].get(variant,"")})
    controls=[]
    for source,text in texts.items():
        if not (source.startswith("fe-ui/src/") or source=="fractalengine/src/terminal_reset/presentation.rs"):
            continue
        for number,line in enumerate(text.splitlines(),1):
            if line.lstrip().startswith("//") or not CONTROL.search(line):
                continue
            controls.append({"source":source,"line":number,"families":sorted(owners.get(source,set())),"candidate":line.strip()[:220]})
    tools=[]
    mcp=texts["fe-api/src/mcp/mod.rs"]
    for match in re.finditer(r"\btool!\(\s*(\w+)\s*,\s*(\w+)\s*,([^\n]+)",mcp):
        name,role,scope=match.groups()
        tools.append({"name":name,"role":role,"scope":scope.rstrip(", "),"checkpoint":"F116.mcp-"+name.replace("_","-"),
                      "source":"fe-api/src/mcp/mod.rs","line":mcp.count("\n",0,match.start())+1})
    routes=[]
    server=texts["fe-api/src/server.rs"]
    for match in re.finditer(r'\.route\(\s*"([^"]+)"',server):
        call,_=balanced(server,match.start()+len(".route"))
        for method in re.findall(r"\b(get|post|put|patch|delete|head|options)\s*\(",call):
            route=match.group(1)
            routes.append({"method":method.upper(),"path":route,"checkpoint":case_id(method,route),
                           "source":"fe-api/src/server.rs","line":server.count("\n",0,match.start())+1})
    if len({(route["method"],route["path"]) for route in routes}) != len(routes):
        raise ValueError("duplicate method/path census")
    unmapped={"actions":[item["action"] for item in actions if not item["families"]],
              "controls":[source for source in sorted({item["source"] for item in controls if not item["families"]})]}
    missing={"tools":[item["checkpoint"] for item in tools if item["checkpoint"] not in {c["id"] for c in catalog["checkpoints"]}],
             "routes":[item["checkpoint"] for item in routes if item["checkpoint"] not in {c["id"] for c in catalog["checkpoints"]}]}
    return {"type":"FeatureSourceCensus","audited_source_commit":snapshot,"execution_evidence":False,
            "pending_source_reconciliation":catalog.get("pending_source_reconciliation",[]),
            "source_acceptance_obligations":catalog.get("source_acceptance_obligations",[]),
            "limits":"Lexical producer candidates, excluding literal cfg(test) items while preserving line numbers. Helpers/named wrappers can repeat one control; conditional production openers require manual reconciliation. Not a Rust parser, live-tree census or proof of execution.",
            "source_sha256":{source:hashlib.sha256(text.encode()).hexdigest() for source,text in texts.items()},
            "actions":actions,"control_candidates":controls,"mcp_tools":tools,"rest_cases":routes,"unmapped":unmapped,"missing_checkpoint_ids":missing}


def render(data):
    lines=["---","type: FeatureSourceInventory","date: 2026-11-02","source_commit: "+data["audited_source_commit"],"execution_status: NOT_RUN","---","",
           "# Source-to-feature reconciliation","",
           "This is an audited source census, not execution evidence. The final staged fingerprint remains pending main integration. Stable checkpoint IDs and retirement reasons live in feature-checkpoints.json. Reproduce with the command in tools/qa/AGENTS.md.","",
           data["limits"],"",
           f"Counts: {len(data['actions'])} UiAction variants; {len(data['control_candidates'])} lexical control/naming candidates; {len(data['mcp_tools'])} MCP tools; {len(data['rest_cases'])} REST method/path cases. Full candidate lines and normalized-source hashes are retained in catalog-source-census.json.","",
           "Candidates map to owning capability families; this is routing responsibility, not an automatic assertion that every branch is covered. A shared helper is not an extra control. Every current mandatory checkpoint still requires current-pass evidence and independent assertions.","",
           "## UiAction producer ownership","","| Action | Families | Current render/gesture producers | Note |","|---|---|---|---|"]
    for item in data["actions"]:
        refs=", ".join("`%s:%s`"%(r["source"],r["line"]) for r in item["render_or_gesture_references"])
        lines.append("| `%s` | %s | %s | %s |"%(item["action"],", ".join(item["families"]),refs or "No render/gesture reference in census",item["note"]))
    lines += ["","## Current control source owners","","| Production source | Families | Lexical candidates |","|---|---|---|"]
    for source in sorted({item["source"] for item in data["control_candidates"]}):
        items=[item for item in data["control_candidates"] if item["source"]==source]
        lines.append("| `%s` | %s | %s |"%(source,", ".join(items[0]["families"]) or "UNMAPPED",len(items)))
    lines += ["","## MCP cases (J09/F116)","","Every available tool requires actual invocation, independent readback, role/scope denial and REST parity where applicable. Feature flags/operator opt-ins are prerequisites; tools/list presence is not PASS.","","| Tool | Role | Scope rule | Checkpoint | Source |","|---|---|---|---|---|"]
    for item in data["mcp_tools"]:
        lines.append("| `%s` | %s | `%s` | `%s` | `%s:%s` |"%(item["name"],item["role"],item["scope"],item["checkpoint"],item["source"],item["line"]))
    lines += ["","## Registered REST method/path cases (J09/F116)","","| Method | Path | Checkpoint | Source |","|---|---|---|---|"]
    for item in data["rest_cases"]:
        lines.append("| %s | `%s` | `%s` | `%s:%s` |"%(item["method"],item["path"],item["checkpoint"],item["source"],item["line"]))
    lines += ["","## Reconciliation limits and explicit gaps","", "Unmapped producer summary: `"+json.dumps(data["unmapped"],sort_keys=True)+"`.","",
              "Missing endpoint checkpoint summary: `"+json.dumps(data["missing_checkpoint_ids"],sort_keys=True)+"`.","",
              "Developer Settings now owns terminal Reset (F086); only a disposable owned store may exercise it. Config URL has an explicit unsupported Inspector explanation and no editor/save route. The old InviteDialog enum/opener is retired. Petal Requirements is declaration-only. PluginHost has no plugin-management GUI. Native Portal fixture evidence (F141) is separate from GUI browsing (F025). Durable REST/legacy WebSocket auth (F142) requires actual lifecycle outcomes, not schema presence.","",
              "Pending source reconciliation: `"+json.dumps(data.get("pending_source_reconciliation",[]))+"`.","",
              "Additional live acceptance obligations: `"+json.dumps(data.get("source_acceptance_obligations",[]))+"`."]
    return "\n".join(lines)+"\n"


def write_catalog_views(root):
    catalog=json.loads((root/"qa/feature-checkpoints.json").read_text(encoding="utf-8-sig"))
    snapshot=catalog["audited_source_commit"]
    counts=Counter(item["reachability"] for item in catalog["checkpoints"])
    path=root/"qa/features.md"
    prefix,body=path.read_text(encoding="utf-8").split("## Capabilities and mandatory variants",1)
    prefix=re.sub(r"(?m)^source_commit: .*", "source_commit: "+snapshot, prefix)
    prefix=re.sub(r"Its \d+ capability families",f"Its {len(catalog['families'])} capability families",prefix)
    prefix=re.sub(r"Audited source `[^`]+` has [^.]+stable checkpoints: [^.]+\.",
                  f"Audited source `{snapshot}` has {len(catalog['families'])} families and {len(catalog['checkpoints'])} stable checkpoints: GUI{counts['Y']}, API/library{counts['A']}, OS{counts['O']}, planned{counts['P']}, unreachable/retired{counts['U']}.",prefix)
    suffix=body.split("## Source reconciliation and exclusions",1)[1]
    rows=["## Capabilities and mandatory variants","","Every suffix is one canonical JSON checkpoint. Current Y/A/O cases are mandatory; P/U are audited exclusions. Repeated instances share mechanisms only with explicit target identity checks. See each owning journey for actions, independent oracles and fixture prerequisites.","",
          "| Family / region / reachability | Concrete entry and checkpoint suffixes | Required observable outcome | Source | Journey |","|---|---|---|---|---|"]
    for family in catalog["families"]:
        items=[item for item in catalog["checkpoints"] if item["family"]==family["id"]]
        reaches=list(dict.fromkeys(item["reachability"] for item in items))
        variants="; ".join(reach+": "+", ".join("`"+item["id"].split(".",1)[1]+"`" for item in items if item["reachability"]==reach) for reach in reaches)
        if family["id"]=="F116":
            variants="Each explicit MCP/REST checkpoint in [source inventory](source-inventory.md); actual invocation/readback, never inventory presence."
        rows.append("| "+family["id"]+" / "+family["region"]+" / "+"/".join(reaches)+" | "+family["entry"]+"<br>"+variants+" | "+family["expected"]+" | "+", ".join("`"+source+"`" for source in family["sources"])+" | ["+family["journey"]+"](scenarios/"+family["journey"]+".md) |")
    path.write_text(prefix+"\n".join(rows)+"\n\n## Source reconciliation and exclusions"+suffix,encoding="utf-8")
    for number in range(1,12):
        journey="J%02d"%number
        path=root/("qa/scenarios/"+journey+".md")
        before,after=path.read_text(encoding="utf-8").split("## Mandatory checkpoint matrix",1)
        lines=after.splitlines()
        start=next(index for index,line in enumerate(lines) if line.startswith("| Family |"))
        end=start
        while end<len(lines) and lines[end].startswith("|"):
            end+=1
        tail="\n".join(lines[end:]).strip()
        matrix=["## Mandatory checkpoint matrix","","Every current Y/A/O suffix needs its own current-pass record. P/U suffixes remain exclusions with reasons in `checkpoint_notes`, never PASS. Audited source `"+snapshot+"`; final staged fingerprint remains pending.","","| Family | Reachability | Required suffixes |","|---|---|---|"]
        for family in catalog["families"]:
            if family["journey"]!=journey:
                continue
            items=[item for item in catalog["checkpoints"] if item["family"]==family["id"]]
            for reach in dict.fromkeys(item["reachability"] for item in items):
                variants=", ".join("`"+item["id"].split(".",1)[1]+"`" for item in items if item["reachability"]==reach)
                if family["id"]=="F116":
                    variants="All explicit MCP/REST IDs in [source inventory](../source-inventory.md); no inventory-only PASS."
                matrix.append("| "+family["id"]+" | "+reach+" | "+variants+" |")
        path.write_text(before+"\n".join(matrix)+"\n"+("\n"+tail+"\n" if tail else ""),encoding="utf-8")


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root",type=Path,default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-commit",required=True)
    parser.add_argument("--write",action="store_true")
    args=parser.parse_args()
    if not re.fullmatch(r"[a-fA-F0-9]{40}",args.source_commit):
        parser.error("source-commit must name the audited full commit")
    root=args.root.resolve()
    data=census(root,args.source_commit)
    if args.write:
        (root/"qa/catalog-source-census.json").write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8")
        (root/"qa/source-inventory.md").write_text(render(data),encoding="utf-8")
        write_catalog_views(root)
    print(json.dumps({"actions":len(data["actions"]),"control_candidates":len(data["control_candidates"]),"mcp_tools":len(data["mcp_tools"]),"rest_cases":len(data["rest_cases"]),"unmapped":data["unmapped"],"missing_checkpoint_ids":data["missing_checkpoint_ids"],"execution_evidence":False}))
    return 2 if any(data["unmapped"].values()) or any(data["missing_checkpoint_ids"].values()) else 0


if __name__=="__main__":
    sys.exit(main())
