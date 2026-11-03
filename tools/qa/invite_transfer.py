"""One-use invitation handoff between pinned owned apps; see AGENTS.md."""

import json
import math
from owned_run_http import OwnedRunTransport


def join_controls(qa,observation):
    lease=observation.get("join_input_lease")
    if not isinstance(lease,dict) or not isinstance(lease.get("lease_id"),str) or not lease["lease_id"].startswith("join-input-"):
        raise qa.Refused(0,"join_control_unavailable","A fresh native masked Join input lease is required")
    points=[]
    for key in ("input_bounds","join_bounds"):
        bounds=lease.get(key)
        if not isinstance(bounds,list) or len(bounds)!=4 or any(type(value) not in (int,float) or not math.isfinite(value) for value in bounds) or bounds[2]<=0 or bounds[3]<=0:
            raise qa.Refused(0,"join_control_unavailable","Guarded Join geometry is unavailable")
        points.append((bounds[0]+bounds[2]/2,bounds[1]+bounds[3]/2))
    if not all(isinstance(lease.get(key),str) and lease[key].isdigit() for key in ("input_widget","join_widget")):
        raise qa.Refused(0,"join_control_unavailable","Opaque native Join widget identities are required")
    return lease,points[0],points[1]


def transfer(qa, owner, args):
    recipient = qa.load_run(args.recipient_run)
    if owner["run_dir"] == recipient["run_dir"] or owner["api_base"] == recipient["api_base"]:
        raise qa.Refused(0, "distinct_owned_runs_required", "Invitation transfer requires two distinct owned apps")
    try:
        transport_context=OwnedRunTransport((owner,recipient))
        return transfer_owned(qa,owner,recipient,args,transport_context)
    except qa.Refused:
        raise
    except Exception:
        raise qa.Refused(0,"owned_transfer_unconfirmed","Owned endpoint or transfer verification failed; no batch may be replayed") from None


def transfer_owned(qa,owner,recipient,args,transport_context):
    with transport_context as transport:
        owner_app,recipient_app=transport.runs
        owner,recipient=owner_app.run,recipient_app.run
        check=transport.check_live
        check()
        owner_verse, owner_token, _ = qa.resolve_token(owner,args.verse,transport=transport,use_cache=False,token_snapshot=owner_app.tokens)
        recipient_verse, recipient_token, _ = qa.resolve_token(recipient,args.recipient_verse,transport=transport,use_cache=False,token_snapshot=recipient_app.tokens)
        check()

        def request(run, method, token, body=None):
            check()
            status, headers, raw = transport(run, method, "/api/v1/debug/invitation", token, body, timeout=5.0)
            check()
            cache_control = next((value for key, value in headers.items() if key.casefold() == "cache-control"), None)
            if not 200 <= status < 300 or cache_control != "no-store":
                raise qa.Refused(status, "invitation_transfer_unconfirmed", "The one-use invitation response was not confirmed; do not replay consumption")
            try:
                envelope = json.loads(raw)
                if envelope.get("ok") is not True or not isinstance(envelope.get("data"), dict):
                    raise ValueError()
                return envelope["data"]
            except (ValueError, TypeError, AttributeError):
                raise qa.Refused(0, "invitation_transfer_unconfirmed", "The one-use response is invalid; do not replay consumption") from None

        owner_observation = request(owner, "GET", owner_token)
        recipient_observation = request(recipient, "GET", recipient_token)
        issuer = owner_observation.get("local_did")
        recipient_did = recipient_observation.get("local_did")
        expected = owner_observation.get("invitation")
        if not isinstance(issuer, str) or not isinstance(recipient_did, str) or issuer == recipient_did or not isinstance(expected, dict):
            raise qa.Refused(0, "addressed_invitation_required", "Generate an addressed invitation for the other owned app first")
        if expected.get("issuer_did") != issuer or expected.get("recipient_did") != recipient_did or expected.get("verse_id") != owner_verse:
            raise qa.Refused(0, "recipient_identity_mismatch", "The current GUI invitation must name the actual other app identity")
        if recipient_observation.get("join_ready_for_input") is not True:
            raise qa.Refused(0, "clean_join_required", "Open a clean, empty Join dialog on the recipient before transferring")
        lease,text_at,join_at=join_controls(qa,recipient_observation)
        check()
        # Recheck both actual DIDs after locating controls, before one-use consumption.
        current_owner = request(owner, "GET", owner_token)
        current_recipient = request(recipient, "GET", recipient_token)
        if current_owner.get("local_did") != issuer or current_owner.get("invitation") != expected or current_recipient.get("local_did") != recipient_did or current_recipient.get("join_ready_for_input") is not True or current_recipient.get("join_input_lease")!=lease:
            raise qa.Refused(0, "recipient_identity_mismatch", "An app identity, invitation generation or clean Join context changed before consumption")
        consumed = request(owner, "POST", owner_token, expected)
        try:
            credential = consumed.get("invitation")
            if consumed.get("metadata") != expected or not isinstance(credential, str) or not credential.startswith("fractalengine://join/v1/") or not credential.isascii() or not 0 < len(credential.encode("utf-8")) <= 65536:
                raise ValueError()
            after_owner = request(owner, "GET", owner_token)
            after_recipient = request(recipient, "GET", recipient_token)
            if after_owner.get("local_did") != issuer or after_recipient.get("local_did") != recipient_did or after_recipient.get("join_ready_for_input") is not True or after_recipient.get("join_input_lease")!=lease:
                raise ValueError()
            commands = [{"kind": "pointer_click", "x": text_at[0], "y": text_at[1], "button": "left"}]
            commands.extend({"kind": "text_entry", "text": credential[index:index+4096]} for index in range(0, len(credential), 4096))
            commands.append({"kind": "pointer_click", "x": join_at[0], "y": join_at[1], "button": "left"})
            body = {"commands": commands, "wait_frames": 0, "capture_after": False}
            if len(json.dumps(body).encode("utf-8")) > 65536:
                raise ValueError()
            check()
            status, _, raw = transport(recipient, "POST", "/api/v1/debug/input", recipient_token, body, timeout=30.0,join_input_lease=lease["lease_id"])
            check()
            if not 200 <= status < 300:
                raise ValueError()
            outcome=json.loads(raw)
            if outcome.get("ok") is not True or outcome.get("commands_applied")!=len(commands):
                raise ValueError()
        except Exception:
            raise qa.Refused(0, "masked_join_submission_unconfirmed", "The invitation was consumed but masked Join submission is unconfirmed; do not replay it") from None
        finally:
            consumed.clear()
            credential = None
            commands = None
            body = None
        return {"transfer": "consumed_once", "input_submitted": True, "commands_applied":outcome["commands_applied"],
                "issuer_did": issuer, "recipient_did": recipient_did,
                "generation": expected["generation"], "scope": expected["scope"], "role": expected["role"],
                "join_ready": "pending_real_transport_and_role_confirmation"}
