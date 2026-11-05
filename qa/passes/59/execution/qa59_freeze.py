import json,hashlib,os,stat
from pathlib import Path
from qa_packet_lib import *
R=Path(__file__).resolve().parents[2];P=R/'qa/passes/59';RUN=R/'.qa/run/20261104-200103-206'
def write(name,value):
 p=P/name;p.parent.mkdir(parents=True,exist_ok=True)
 with p.open('x',encoding='utf-8') as h:json.dump(value,h,indent=2,ensure_ascii=True);h.write('\n')
originals=manifest_files(P);copies=[]
def cp(src,name):
 dst=P/name;data=copy_exact(src,dst);copies.append({'source':str(src),'file':name,**describe_file(dst)})
for src in sorted(P.glob('owner-*-shot.jsonl')):
 result=json.loads(src.read_text(encoding='utf-8'));image=Path(result['path']);assert image.parent==RUN/'shots';cp(image,'screenshots/'+image.name)
for name in ['qa59_flow.py','qa59_matrix.py','qa59_functional.py','qa59_mutations.py','qa59_readability.py','qa59_freeze.py','qa_pair_drive.py','qa_packet_lib.py','check_qa59_cleanup.ps1']:
 cp(R/'.omc/research'/name,'execution/'+name)
for name in ['qa59-owned-context.json','qa59-cleanup-identities.json','qa59-launch.log','qa59-stop.log','completion-readable-grid-provider-stage-receipt.json','completion-readable-grid-provider-stage.log']:
 cp(R/'.omc/research'/name,'provenance/'+name)
cp(R/'.qa/stage/be081abe-20261104194142170/stage.json','provenance/stage.json')
logs={n:describe_log(RUN/n) for n in ['stdout.log','stderr.log']}
write('app-log-review.json',{'raw_logs_private':True,'logs':logs,'warning_classes':{'startup':9,'synthetic_clipboard_suppression':1},'actual_warn_total':10,'error_total':0,'limits':'Finite marker/credential checks are not universal absence proof. Clipboard suppression prevents actual host clipboard acceptance.'})
scans=[]
for p in sorted(P.rglob('*')):
 if p.is_file() and p.suffix!='.png':
  t,enc=decode_text(p.read_bytes());counts=scan(t);assert not any(counts.values()),p
  scans.append({'file':p.relative_to(P).as_posix(),'encoding':enc,'counts':counts})
write('credential-scan.json',{'patterns':PATTERNS,'files':scans,'private_logs':{n:x['credential_pattern_counts'] for n,x in logs.items()},'limits':'Exactly four patterns over listed strict-decoded files/private logs only. This scan predates its own output, packet summary and manifest. No universal absence claim.'})
assert all(describe_file(P/x['file'])=={'bytes':x['bytes'],'sha256':x['sha256']} for x in originals)
write('packet-summary.json',{'kind':'qa59-focused-rebuilt-property-grid','source_commit':'be081abed05be3c623b93bef33d4dd31d43d9b46','stage':'.qa/stage/be081abe-20261104194142170','original_anchors':originals,'external_copies':copies,'matrix':{'six_canonical_size_scale_cases_pass_whole_numeric_row_actions':True,'window_sizes':[[1024,720],[1280,720],[1600,900]],'scales':[1,2],'body_logical_widths':[260,260,260,67,79,167],'layout_check_each_case':True,'layout_coverage_complete':False,'measured_violations':0},'functional':{'probe_node_id':'01M6MTJZSA18WHER52WH3192XA','draft_raw':'-150.250','both_cancels_leave_version1_unchanged':True,'one_gui_save_version2':True,'one_gui_confirmed_delete_version3':True,'copy_toast_observed_storage_unchanged':True,'clipboard_contents_verified':False,'exact_full_state_and_prior_raw_history_prefix_verified':True,'integer':9007199254740993,'long_and_greek_keys_actual_screenshots':True},'actual_width_limits':{'requested_520_at_1x_actual_260':True,'requested_52_at_2x_actual_67':True,'live_52_or_520_acceptance':False},'root_visual_inspection':{'actual_pngs_inspected':17,'residual':'Square icon placeholders observed in toolbar/Inspector/tree controls; separate visual/source review pending. Actual long and Greek narrow keys painted in screenshots. Extreme narrow UI requires vertical scrolling.'},'owned_cleanup':{'recorded_identities':8,'all_absent_at':'2026-11-05T03:10:20.1725651Z'},'operator_refusal':'Initial ambiguous Petal A locator refused before probe creation; explicit visible tree candidate then succeeded. Refusal retained.','limits':['Focused GUI/data evidence only; no FULL or universal visual acceptance.','No actual mesh/asset/picking, peer, retained restart, native UIA or capture-freshness acceptance.','No tests/fmt/final integrated sweep/CI/tag/release executed.','Source stage dirty flag includes unrelated session/evidence state; stage_matches_source true and exact build-input fingerprint retained.','Frozen bytes must not be rewritten; corrections belong in external clarifications.']})
manifest={'kind':'qa59-packet-manifest','files':manifest_files(P)};write('packet-manifest.json',manifest)
receipt={'kind':'qa59-packet-freeze','manifest':describe_file(P/'packet-manifest.json'),'payload_count':len(manifest['files']),'png_count':len(list(P.rglob('*.png'))),'original_anchor_count':len(originals),'source_copy_count':len(copies),'source_or_original_mismatches':0,'note':'Frozen file-only packet; independent evidence review pending.'}
(R/'.omc/research/qa59-packet-freeze-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n',encoding='utf-8')
for p in P.rglob('*'):
 if p.is_file():os.chmod(p,stat.S_IREAD)
print(json.dumps(receipt))
