import hashlib,json
from pathlib import Path
from qa60_flow import *
(PACKET/'execution').mkdir(exist_ok=True)
for name in ['qa60_matrix.py','qa60_flow.py']:
 data=Path(__file__).with_name(name).read_bytes()
 with (PACKET/'execution'/name).open('xb') as h:h.write(data)
def align(label,name,region):
 t=ui(label+'-before-ui');n=named(t,name);clip=scroll_region(t,region)['clip'];desired=clip['y']+8
 if region=='Hierarchy':desired=clip['y']+clip['h']*0.7
 delta=(desired-n['bounds']['y'])/(40*t['pixels_per_point'])
 drive(label+'-scroll','scroll',str(delta),str(clip['x']+4),str(clip['y']+clip['h']/2))
 return ui(label+'-after-ui')
def toolbar(t,width):
 limit=40*t['pixels_per_point'];nodes=[n for n in t['nodes'] if n.get('role')=='button' and not n.get('hidden') and n.get('bounds') and n['bounds']['y']<limit and n['bounds']['y']+n['bounds']['h']>0]
 overlapping=[]
 for i,a in enumerate(nodes):
  for b in nodes[i+1:]:
   x,y=a['bounds'],b['bounds'];w=min(x['x']+x['w'],y['x']+y['w'])-max(x['x'],y['x']);h=min(x['y']+x['h'],y['y']+y['h'])-max(x['y'],y['y'])
   if w>0.6 and h>0.6:overlapping.append({'a':a['name'],'b':b['name'],'overlap_width':w,'overlap_height':h,'a_bounds':x,'b_bounds':y})
 outside=[n for n in nodes if not contains({'x':0,'y':0,'w':width,'h':limit},n['bounds'])]
 return {'nodes':nodes,'overlaps':overlapping,'outside':outside,'pass':not overlapping and not outside,'limits':'Semantic button rectangles only; painted icon shape/occlusion requires actual screenshot review.'}
cases=[]
for w,h,scale in [(1024,720,1),(1280,720,1),(1600,900,1),(1024,720,2),(1280,720,2),(1600,900,2),(1159,720,1),(1160,720,1),(1161,720,1)]:
 label=f'matrix-{w}-scale{scale}';drive(label+'-size','window','size',str(w),str(h));drive(label+'-scale','window','ui-scale',str(scale));state=drive(label+'-state','state')[-1]
 assert state['window']['width']==w and state['window']['height']==h and state['window']['ui_scale']==scale
 assert state['selection']['node_id']==create_result()['node_id']
 t=align(label+'-tree','Select node QA60 icon probe','Hierarchy');node=named(t,'Select node QA60 icon probe');treefact={'node':node,'clip':scroll_region(t,'Hierarchy')['clip'],'whole_visible':contains(scroll_region(t,'Hierarchy')['clip'],node['bounds'])}
 t=align(label+'-number','grid_number','Inspector');facts=record_actions(t,['Edit property grid_number','Copy property grid_number','Delete property grid_number']);facts['topbar']=toolbar(t,w);facts['selected_tree']=treefact
 layout=drive(label+'-layout','layout-check')[-1];shot=drive(label+'-shot','shot','--label','qa60-'+label)[-1]
 facts.update(case=label,window=state['window'],screenshot=shot['path'],layout_check={'coverage_complete':layout['coverage_complete'],'violations':layout['violations']})
 cases.append(facts);save(label+'-comparison.json',facts)
 print(json.dumps({'case':label,'actions_fit':facts['whole_targets_fit'],'toolbar_fit':facts['topbar']['pass'],'overlaps':[(x['a'],x['b']) for x in facts['topbar']['overlaps']],'selected_row_visible':treefact['whole_visible']}),flush=True)
save('icons-responsive-matrix.json',{'source_commit':'9498869e2e1577e939a10f0a1e0f9e07de3b7053','cases':cases,'limits':'Failures retained. Actual screenshots need inspection. No data mutations, native UIA, peer, FULL or release acceptance.'})
