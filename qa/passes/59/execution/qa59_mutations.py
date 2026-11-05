import copy,json
from qa59_flow import *
BASE=create_result(); ID=BASE['node_id']
expected=copy.deepcopy(BASE['state']['properties'])
drive('save-size','window','size','1280','720');drive('save-scale','window','ui-scale','1');resize_body('save-body',260)
scroll_to('save-editor-view','Edit property grid_number')
drive('save-edit','click','--name','Edit property grid_number','--role','button')
drive('save-focus','click','--name','Value of property grid_number','--role','multilinetextinput')
drive('save-select-all','key','ctrl+a');drive('save-type','type','-150.250')
t=ui('save-draft-ui');assert named(t,'Value of property grid_number')['value']=='-150.250'
scroll_to('save-button-view','Save property grid_number')
drive('save-once','click','--name','Save property grid_number','--role','button')
p2=api('properties-after-save','GET',f'/api/v1/nodes/{ID}/properties')
h2=api('history-after-save','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')
expected['grid_number']=-150.25
assert p2['properties']==expected and h2['current_version']==2
assert h2['entries'][0]==json.loads((PACKET/'owner-history-before.jsonl').read_text(encoding='utf-8'))['data']['entries'][0]
drive('saved-ui','ui');drive('saved-shot','shot','--label','qa59-saved')
scroll_to('delete-view','Delete property grid_remove')
drive('delete-arm','click','--name','Delete property grid_remove','--role','button')
t=ui('delete-armed-ui');save('delete-confirm-targets.json',observe_actions(t,['Confirm Delete property grid_remove','Cancel Delete property grid_remove']))
drive('delete-armed-shot','shot','--label','qa59-delete-confirm')
drive('delete-confirm-once','click','--name','Confirm Delete property grid_remove','--role','button')
p3=api('properties-after-delete','GET',f'/api/v1/nodes/{ID}/properties')
h3=api('history-after-delete','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')
expected.pop('grid_remove')
assert p3['properties']==expected and h3['current_version']==3
assert h3['entries'][:2]==h2['entries']
state=api('state-after-delete','GET',f'/api/v1/nodes/{ID}/state?at_version=3')
drive('deleted-ui','ui');drive('deleted-shot','shot','--label','qa59-deleted')
scroll_to('copy-view','Copy property grid_text')
drive('copy-once','click','--name','Copy property grid_text','--role','button')
t=ui('copied-ui');drive('copied-shot','shot','--label','qa59-copy-feedback')
assert any('Copied grid_text' in n.get('name','') for n in t['nodes'])
assert api('properties-after-copy','GET',f'/api/v1/nodes/{ID}/properties')==p3
assert api('history-after-copy','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')==h3
save('mutation-oracle.json',{'node_id':ID,'save_version':2,'delete_version':3,'expected_properties':expected,'only_number_changed_at_save':True,'only_remove_deleted':True,'prior_entries_unchanged':True,'copy_feedback_observed':True,'copy_storage_unchanged':True,'clipboard_contents_verified':False,'historical_state_response':state})
