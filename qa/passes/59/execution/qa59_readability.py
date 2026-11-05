import copy,json
from qa59_flow import *
drive('keys-size','window','size','1024','720');drive('keys-scale','window','ui-scale','2');resize_body('keys-minimum',52)
facts=[]
for label,name in [('number','grid_number'),('long','long_property_key_without_breaks_for_responsive_readability'),('unicode','sensor_\u03b1_\u03b2_\u03b3')]:
 t=ui('keys-'+label+'-before-ui');n=named(t,name);clip=scroll_region(t,'Inspector')['clip']
 desired=clip['y']+8
 delta=(desired-n['bounds']['y'])/(40*t['pixels_per_point'])
 drive('keys-'+label+'-scroll','scroll',str(delta),str(clip['x']+4),str(clip['y']+clip['h']/2))
 t=ui('keys-'+label+'-after-ui');n=named(t,name)
 assert contains(scroll_region(t,'Custom property table')['clip'],n['bounds'])
 shot=drive('keys-'+label+'-shot','shot','--label','qa59-keys-'+label)[-1]
 facts.append({'key':name,'node':n,'clip':scroll_region(t,'Custom property table')['clip'],'screenshot':shot['path'],'body_logical_width':scroll_region(t,'Custom property table')['viewport']['w']/t['pixels_per_point']})
save('readability-facts.json',{'cases':facts,'limits':'Whole semantic label bounds do not independently prove painted glyph coverage; actual screenshots require inspection.'})
baseline=create_result()['state'];actual=json.loads((PACKET/'owner-state-after-delete.jsonl').read_text(encoding='utf-8'))['data']['state']
expected=copy.deepcopy(baseline);expected['properties']['grid_number']=-150.25;expected['properties'].pop('grid_remove')
assert actual==expected
assert isinstance(actual['properties']['grid_payload']['samples'][2]['sequence'],int)
assert actual['properties']['grid_payload']['samples'][2]['sequence']==9007199254740993
assert actual['properties']['grid_text']=='  exact\ntext  '
def raw_entries(path):
 raw=path.read_text(encoding='utf-8');start=raw.index('"entries":')+len('"entries":');i=raw.index('[',start)+1;decoder=json.JSONDecoder();parts=[]
 while True:
  while raw[i].isspace() or raw[i]==',':i+=1
  if raw[i]==']':return parts
  _,end=decoder.raw_decode(raw,i);parts.append(raw[i:end]);i=end
h1=raw_entries(PACKET/'owner-history-before.jsonl');h2=raw_entries(PACKET/'owner-history-after-save.jsonl');h3=raw_entries(PACKET/'owner-history-after-delete.jsonl')
assert h2[:1]==h1 and h3[:2]==h2
save('complete-state-oracle.json',{'full_state_equals_baseline_with_only_two_intended_property_changes':True,'prior_raw_entry_substrings_byte_identical':True,'exact_large_integer':9007199254740993,'whitespace_string_exact':True,'historical_version':3})
