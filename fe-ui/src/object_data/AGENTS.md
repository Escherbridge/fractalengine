# Native object data

`ObjectDataState` is an Inspector read model. Selection remains in NodeManager; navigation remains in
NavigationManager. A target consists of the selected node and its complete current ancestry. Target
changes, unresolved access or a role below Viewer (including `Some(RoleLevel::None)`) clear pending
requests and cached views. Stable targets avoid per-frame
identity allocation. Every correlated request has a fresh `ui-*` ID and a 30-second timeout.

`history.rs` consumes only scoped history/replay results. Commit versions order one object; they do
not define a global wall-clock cut. Pages replace one bounded 50-fact page, and navigation keeps at
most 64 cursors. Exact-version detail and timed replay are read-only previews. They never write
VerseManager, ECS transforms or current Inspector properties. Unsupported, incomplete and inaccessible
intervals remain explicit. JSON detail is formatted when a response arrives, rather than every frame.

`measurements.rs` draws real persisted measurements, separately grouped by metric and units. Event
timestamps determine chart/table order, with reading ID as a deterministic tie breaker. The runtime
owns the shared 64-series/1,024-point bounds. Partial series are labelled as the earliest bounded
points, with later points omitted. Finite values are normalized before plotting to prevent overflow
even when their difference would exceed `f64`. The timestamp/value table and textual summary are the
accessible equivalents of the painter chart. TSV copies exactly the displayed points and labels.

`schema.rs` selects existing definitions at the current Verse, Fractal or Petal scope. It reads and
edits the bounded enforced descriptor, separately from legacy type/required/default metadata. The
shared SDK parser rejects unsupported keywords, invalid bounds and out-of-range integers before
dispatch. The database remains authoritative for role, scope, next revision and compatibility.
Saving waits for the returned descriptor to equal the submitted descriptor. Errors and timeouts
retain the draft; ambiguous outcomes require a stored readback before another save. A concurrent
revision preserves the draft and asks the user to load current rules before saving.

Each submitted schema attempt retains both its descriptor and exact submitted text. A later readback
can confirm that attempt without replacing edits made after submission. Only an unchanged submitted
draft resets to the next revision template. Newer or partially edited text remains intact, with the
required next revision shown and validated before another save. Deadline evaluation takes a monotonic
instant so existing journeys can exercise timeouts without sleeping.

Ordinary GUI property writes retain their older uncorrelated command contract. Their backend errors
appear in the persistent notice/history surface; the typed editor retains drafts and ends an
unconfirmed pending write after 30 seconds. Never attach an arbitrary global error to a property key.
