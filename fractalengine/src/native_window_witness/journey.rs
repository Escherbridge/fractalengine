//! Existing pure App lifecycle extension, with no native handle construction.

use super::*;

pub(crate) fn extend_existing_journey() {
    let mut app = App::new();
    observe_lifetimes(&mut app);
    app.init_resource::<Witness>();
    app.init_resource::<Lifecycle>()
        .add_message::<AppLifecycle>()
        .add_systems(Update, observe_lifecycle);
    let before = book::revision().unwrap();
    let entity = app.world_mut().spawn(EguiContext::default()).id();
    let inserted = book::revision().unwrap();
    assert!(
        inserted > before,
        "actual Context insertion invalidates before publication"
    );
    app.world_mut()
        .entity_mut(entity)
        .insert(EguiContext::default());
    let replaced = book::revision().unwrap();
    assert!(
        replaced > inserted,
        "same Entity context replacement cannot retain an epoch"
    );
    app.world_mut().entity_mut(entity).remove::<EguiContext>();
    let removed = book::revision().unwrap();
    assert!(removed > replaced, "removal invalidates the old context");
    let old = book::fresh().unwrap();
    retire(app.world_mut());
    app.world_mut().insert_resource(Witness::default());
    let after_reset = book::fresh().unwrap();
    assert!(
        after_reset > old,
        "read-model reset cannot reset process lifetime allocation"
    );
    assert!(book::begin_frame().is_some());
    assert!(
        !book::publish(removed, 0, 0),
        "a revision superseded by further component changes cannot publish"
    );
    assert!(
        book::successor(isize::MAX as u64).is_none(),
        "counter exhaustion never wraps"
    );
    assert!(book::successor(u64::MAX).is_none());
    assert_eq!(windows::owns(0, 7), Ok(false));
    assert_eq!(windows::owns(7, 7), Ok(true));
    assert!(
        windows::owns(8, 7).is_err(),
        "foreign property cannot be removed as ours"
    );

    let context = app.world_mut().spawn(EguiContext::default()).id();
    assert!(!app.world().resource::<Lifecycle>().running);
    app.world_mut().write_message(AppLifecycle::Running);
    app.update();
    assert!(app.world().resource::<Lifecycle>().running);
    let before_suspend = book::revision().unwrap();
    let lifetime = book::fresh().unwrap();
    app.world_mut().write_message(AppLifecycle::Suspended);
    app.update();
    assert!(!app.world().resource::<Lifecycle>().running);
    assert!(book::revision().unwrap() > before_suspend,
        "actual suspend message retires witness even when the Context survives");
    assert!(app.world().get::<EguiContext>(context).is_some());
    app.world_mut().write_message(AppLifecycle::WillResume);
    app.update();
    assert!(!app.world().resource::<Lifecycle>().running,
        "WillResume is not an already completed Running pass");
    app.world_mut().write_message(AppLifecycle::Running);
    app.update();
    assert!(app.world().resource::<Lifecycle>().running);
    assert!(book::fresh().unwrap() > lifetime,
        "same-context resume cannot reuse the retired binding epoch");
    assert!(app.world().resource::<Witness>().prepared.is_none(),
        "running lifecycle alone never fabricates a completed output");
}
