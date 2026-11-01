# Sync integration tests

`relay_assisted_test.rs::skip_unless_required` owns the infrastructure
availability decision. Every runtime/container/config, endpoint setup and
provider relay-reachability skip must call it. Optional developer runs log a
skip; `FE_REQUIRE_RELAY_CONTAINER=1` makes an unexercised acceptance path fail.
Expected negative authentication and protocol assertions remain ordinary test
assertions: a missing endpoint is not evidence that a bad token was refused.
