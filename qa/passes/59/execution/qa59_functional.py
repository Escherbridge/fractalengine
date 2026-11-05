import json
from qa59_flow import *
BASE=create_result(); ID=BASE['node_id']
props0=api('properties-before','GET',f'/api/v1/nodes/{ID}/properties')
history0=api('history-before','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')
drive('edit-size','window','size','1600','900');drive('edit-scale','window','ui-scale','1')
resize_body('edit-wide',520)
scroll_to('edit-number','Edit property grid_number')
drive('edit-number-click','click','--name','Edit property grid_number','--role','button')
drive('draft-focus','click','--name','Value of property grid_number','--role','multilinetextinput')
drive('draft-select-all','key','ctrl+a');drive('draft-type','type','-150.250')
t=ui('draft-wide-ui')
assert named(t,'Value of property grid_number')['value']=='-150.250'
drive('draft-wide-shot','shot','--label','qa59-draft-wide')
drive('draft-narrow-size','window','size','1024','720');drive('draft-narrow-scale','window','ui-scale','2')
resize_body('draft-narrow',52)
t=scroll_to('draft-narrow-editor','Value of property grid_number')
assert named(t,'Value of property grid_number')['value']=='-150.250'
drive('draft-narrow-shot','shot','--label','qa59-draft-narrow')
scroll_to('draft-cancel-view','Cancel editing property grid_number')
drive('draft-cancel','click','--name','Cancel editing property grid_number','--role','button')
assert api('properties-after-cancel','GET',f'/api/v1/nodes/{ID}/properties')==props0
assert api('history-after-cancel','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')==history0
scroll_to('narrow-delete-view','Delete property grid_remove')
drive('narrow-arm-delete','click','--name','Delete property grid_remove','--role','button')
t=ui('narrow-delete-armed-ui')
save('narrow-delete-targets.json',observe_actions(t,['Confirm Delete property grid_remove','Cancel Delete property grid_remove']))
drive('narrow-delete-armed-shot','shot','--label','qa59-narrow-confirm-cancel')
drive('narrow-cancel-delete','click','--name','Cancel Delete property grid_remove','--role','button')
assert api('properties-after-delete-cancel','GET',f'/api/v1/nodes/{ID}/properties')==props0
assert api('history-after-delete-cancel','GET',f'/api/v1/nodes/{ID}/history?after_version=0&limit=64')==history0
save('cancel-oracle.json',{'node_id':ID,'draft_raw':'-150.250','draft_survived_resize':True,'property_and_history_unchanged_after_both_cancels':True})
