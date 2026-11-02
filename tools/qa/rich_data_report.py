"""Export actual collected rich-data values; see AGENTS.md §rich-data-report."""

import csv
import json


def export_report(folder, context):
    reverse = {node_id:name for name,node_id in context["objects"].items()}
    readings = {}
    for query in context["queries"].values():
        if query["kind"] != "rows":
            continue
        for row in query["rows"]:
            key = row.get("reading_id")
            if key:
                readings[key] = {"object":reverse.get(row["node_id"],"unknown"), **row}
    ordered = sorted(readings.values(), key=lambda row:(row["recorded_at_ms"],row["object"],row["metric"]))
    (folder / "measurements.json").write_text(json.dumps(ordered,ensure_ascii=False,indent=2,allow_nan=False)+"\n",encoding="utf-8")
    columns = ("object","node_id","metric","recorded_at_ms","value","units","payload_id","reading_id")
    with (folder / "measurements.csv").open("x", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle,fieldnames=columns,extrasaction="ignore")
        writer.writeheader()
        writer.writerows(ordered)
    with (folder / "trajectory.csv").open("x", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(("object","node_id","history_version","x","y","z"))
        for name, points in context["trajectories"].items():
            for point in points:
                writer.writerow((name,context["objects"][name],point["version"],*point["position"]))
    histories = {name:{} for name in context["objects"]}
    for name, entries in context["history"].items():
        for entry in entries:
            state = entry.get("payload",{}).get("after_state")
            if isinstance(state,dict):
                histories[name][str(entry["row_version"])] = json.dumps(state,ensure_ascii=False,indent=2)
    for snapshot in context["checkpoints"].values():
        for name, item in snapshot.items():
            histories[name][str(item["history_version"])] = json.dumps(item["state"],ensure_ascii=False,indent=2)
    for name,state in context["states"].items():
        histories[name][str(context["versions"][name])] = json.dumps(state,ensure_ascii=False,indent=2)
    model = {"provenance":context["provenance"],"complete":context["data_assertions_succeeded"],
             "log_review_required":context["log_review_required"],"readings":ordered,
             "trajectories":context["trajectories"],"histories":histories}
    encoded = json.dumps(model,ensure_ascii=False,allow_nan=False).replace("<","\\u003c").replace("\u2028","\\u2028").replace("\u2029","\\u2029")
    page = HTML.replace("__REPORT_DATA__",encoded)
    (folder / "report.html").write_text(page,encoding="utf-8")
    return ("measurements.json","measurements.csv","trajectory.csv","report.html")


HTML = '''<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Rich data QA evidence</title>
<style>
body{font:16px system-ui,sans-serif;margin:0;background:#10151f;color:#e1e8f4}main{max-width:1160px;margin:auto;padding:32px}h1{margin-bottom:8px}h2{font-size:21px}p{color:#acb9cb;line-height:1.5}.card{background:#182231;border:1px solid #2f4057;border-radius:12px;padding:24px;margin:20px 0}label{display:inline-flex;gap:9px;align-items:center;margin-right:20px}select{font:inherit;background:#0d1725;color:#e1e8f4;border:1px solid #456079;border-radius:6px;padding:7px}svg{width:100%;height:300px;display:block;margin-top:14px;background:#101a28;border-radius:8px}a{color:#81c8ff}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#101a28;padding:16px;border-radius:8px;font-size:13px}table{border-collapse:collapse;width:100%;font-size:13px}th,td{text-align:left;border-bottom:1px solid #304158;padding:8px}.scroll{overflow:auto;max-height:340px}.legend{font-size:13px;color:#acb9cb}.status{font-weight:600;color:#ffcb7d}
</style><main>
<h1>Rich data simulation evidence</h1>
<p>This offline report is generated from actual collected API rows and committed object-history snapshots. It is a QA artifact, not the application's chart UI. Event-time measurements and per-object commit versions are separate axes.</p>
<p id="status" class="status"></p><p><a href="measurements.csv">Measurement CSV</a> · <a href="trajectory.csv">Trajectory CSV</a> · <a href="context.json">Full evidence context JSON</a></p>
<section class="card"><h2>Appended measurements</h2><label>Metric <select id="metric"></select></label><svg id="measurements" viewBox="0 0 1000 300" role="img" aria-label="Actual measurements over sensor event time"></svg><p class="legend">Alpha: cyan · Beta: amber · Gamma: green. X uses recorded_at_ms; late arrival does not move an earlier event to the end.</p><div id="reading-table" class="scroll"></div></section>
<section class="card"><h2>Stored object trajectory</h2><svg id="trajectory" viewBox="0 0 1000 300" role="img" aria-label="XZ projection of committed object positions"></svg><p class="legend">XZ projection only. The table retains Y; travel assertions use full XYZ distances. Rotation and scale updates do not add translation distance.</p><div id="trajectory-table" class="scroll"></div></section>
<section class="card"><h2>Historical object state</h2><label>Object <select id="object"></select></label><label>Committed version <select id="version"></select></label><pre id="state"></pre><p>Snapshots are formatted before browser parsing, preserving exact large integer text. A version map is an explicit per-object cut, not a global wall-time snapshot.</p></section>
<section class="card"><h2>Source provenance</h2><pre id="provenance"></pre><p>One host's complete query does not prove replicated durability. Affine calibration is an explicit driver operation using the selected historical calibration, not an automatic product rule engine.</p></section>
</main><script id="report-data" type="application/json">__REPORT_DATA__</script>
<script>
const data=JSON.parse(document.getElementById('report-data').textContent);
const colors=['#69d7ef','#ffbd67','#91dfa8','#bca1ff'];
const names=Object.keys(data.histories);
const $=id=>document.getElementById(id);
function text(parent,tag,value){const node=document.createElement(tag);node.textContent=String(value??'');parent.appendChild(node);return node;}
function options(select,values){select.replaceChildren();for(const value of values){const item=document.createElement('option');item.value=value;item.textContent=value;select.appendChild(item);}}
function table(target,headers,rows){target.replaceChildren();const t=text(target,'table','');const head=text(t,'thead','');const hr=text(head,'tr','');for(const h of headers)text(hr,'th',h);const body=text(t,'tbody','');for(const row of rows){const tr=text(body,'tr','');for(const value of row)text(tr,'td',value);}}
function svgNode(svg,tag,attributes,label){const node=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const [key,value] of Object.entries(attributes))node.setAttribute(key,String(value));if(label!==undefined)node.textContent=String(label);svg.appendChild(node);return node;}
function plot(svg,series,xLabel,yLabel){svg.replaceChildren();const points=series.flatMap(s=>s.points);if(!points.length){svgNode(svg,'text',{x:45,y:65,fill:'#acb9cb'},'No actual rows were collected for this view.');return;}let xmin=Math.min(...points.map(p=>p[0])),xmax=Math.max(...points.map(p=>p[0])),ymin=Math.min(...points.map(p=>p[1])),ymax=Math.max(...points.map(p=>p[1]));if(xmax===xmin)xmax=xmin+1;if(ymax===ymin)ymax=ymin+1;const x=v=>65+880*(v-xmin)/(xmax-xmin),y=v=>245-200*(v-ymin)/(ymax-ymin);svgNode(svg,'path',{d:'M65 35V245H945',fill:'none',stroke:'#53667f'});for(const s of series){svgNode(svg,'polyline',{points:s.points.map(p=>x(p[0])+','+y(p[1])).join(' '),fill:'none',stroke:s.color,'stroke-width':2});for(const p of s.points){const circle=svgNode(svg,'circle',{cx:x(p[0]),cy:y(p[1]),r:4,fill:s.color});const title=document.createElementNS('http://www.w3.org/2000/svg','title');title.textContent=s.name+': '+p[0]+', '+p[1];circle.appendChild(title);}}svgNode(svg,'text',{x:65,y:275,fill:'#acb9cb'},xLabel+' ['+xmin+' … '+xmax+']');svgNode(svg,'text',{x:65,y:24,fill:'#acb9cb'},yLabel+' ['+ymin+' … '+ymax+']');}
function drawMeasurements(){const metric=$('metric').value;const rows=data.readings.filter(r=>r.metric===metric);plot($('measurements'),names.map((name,i)=>({name,color:colors[i%colors.length],points:rows.filter(r=>r.object===name).sort((a,b)=>a.recorded_at_ms-b.recorded_at_ms).map(r=>[r.recorded_at_ms,r.value])})),'Sensor UTC epoch ms',metric);table($('reading-table'),['Object','Sensor time UTC','Value','Units','Reading ID'],rows.map(r=>[r.object,new Date(r.recorded_at_ms).toISOString(),r.value,r.units,r.reading_id]));}
function drawState(){const history=data.histories[$('object').value]||{};options($('version'),Object.keys(history).sort((a,b)=>Number(a)-Number(b)));showState();}
function showState(){$('state').textContent=(data.histories[$('object').value]||{})[$('version').value]||'No historical snapshot collected.';}
$('status').textContent=(data.complete?'All declared data assertions completed.':'Execution incomplete or failed; do not treat this as a passing simulation.')+(data.log_review_required?' Warning/error evidence still needs independent review.':'');
options($('metric'),[...new Set(data.readings.map(r=>r.metric))]);$('metric').addEventListener('change',drawMeasurements);drawMeasurements();
plot($('trajectory'),names.map((name,i)=>({name,color:colors[i%colors.length],points:(data.trajectories[name]||[]).map(p=>[p.position[0],p.position[2]])})),'X world meters','Z world meters');
table($('trajectory-table'),['Object','Commit version','X','Y','Z'],Object.entries(data.trajectories).flatMap(([name,points])=>points.map(p=>[name,p.version,...p.position])));
options($('object'),names);$('object').addEventListener('change',drawState);$('version').addEventListener('change',showState);drawState();$('provenance').textContent=JSON.stringify(data.provenance,null,2);
</script></html>'''
