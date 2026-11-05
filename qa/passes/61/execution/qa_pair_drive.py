import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys

root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'tools/qa'))
spec=importlib.util.spec_from_file_location('fe',root/'tools/qa/fe.py')
fe=importlib.util.module_from_spec(spec)
sys.modules[spec.name]=fe
spec.loader.exec_module(fe)
pass_id,side,label=sys.argv[1:4]
assert pass_id.isascii() and pass_id.isdigit() and side in ('owner','recipient')
assert label.isascii() and all(c.isalnum() or c in '-_' for c in label)
context=json.loads((root/f'.omc/research/qa{pass_id}-owned-context.json').read_text(encoding='utf-8-sig'))
arguments=json.load(sys.stdin)
assert isinstance(arguments,list) and arguments
if arguments[:2]==['api','POST'] and len(arguments)>2 and arguments[2].split('?')[0]=='/api/v1/debug/invitation':
    raise SystemExit('Credential consumption must use the reviewed transfer-invite command')
if arguments==['transfer-to-recipient']:
    assert side=='owner'
    arguments=['transfer-invite','--recipient-run',context['recipient']['run_dir']]
folder=root/f'qa/passes/{pass_id}'
folder.mkdir(exist_ok=True)
path=folder/(side+'-'+label+'.jsonl')
if path.exists():raise SystemExit('Evidence exists; do not repeat mutation or credential transfer')
output=io.StringIO()
with contextlib.redirect_stdout(output):
    code=fe.main(['--run',context[side]['run_dir'],*arguments])
path.write_text(output.getvalue(),encoding='utf-8')
values=[json.loads(line) for line in output.getvalue().splitlines() if line.strip()]
print(json.dumps({'side':side,'step':label,'exit':code,'artifact':str(path.relative_to(root)),
 'results':[{k:v[k] for k in ('refused','code','message','path','capture_path') if k in v} for v in values]}))
raise SystemExit(code)
