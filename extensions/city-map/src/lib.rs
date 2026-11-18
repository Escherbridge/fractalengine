//! city-map: minimal `petal.map` provider without scene writes for scoped binding QA.

use fe_ext_guest::{extension, Extension, HookCall, HookFailure, HookOutput, TickInfo};

const PETAL_MAP_HOOK: &str = "petal.map";

struct CityMap;

impl Extension for CityMap {
    fn on_tick(_info: TickInfo) {}

    fn on_hook(call: HookCall) -> Result<HookOutput, HookFailure> {
        if call.hook != PETAL_MAP_HOOK {
            return Err(HookFailure::Failed(format!(
                "hook {} has no handler in this extension",
                call.hook
            )));
        }
        Ok(HookOutput::json(serde_json::json!({ "terrain": null })))
    }
}

extension!(CityMap);
