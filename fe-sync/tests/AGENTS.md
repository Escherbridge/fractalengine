# Sync integration tests

`relay_assisted_test.rs::skip_unless_required` owns the infrastructure
availability decision. Every runtime/container/config, endpoint setup and
provider relay-reachability skip must call it. Optional developer runs log a
skip; `FE_REQUIRE_RELAY_CONTAINER=1` makes an unexercised acceptance path fail.
Expected negative authentication and protocol assertions remain ordinary test
assertions: a missing endpoint is not evidence that a bad token was refused.

## peer_relay_e2e_test.rs

Peer-relay end-to-end evidence (P15). Two relay-only peers reach each other through an in-process, members-only relay (`run_relay_server_with_access` with `RelayMembershipGate`, feature `relay-server`). The relay is real; only the clock and DNS are faked. One journey per acceptance cluster. It has its own slow line in `conductor/test-batches.md` (`cargo test -p fe-sync --features relay-server --test peer_relay_e2e_test`) and runs only when named.
