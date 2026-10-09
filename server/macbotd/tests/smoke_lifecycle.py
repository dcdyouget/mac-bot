#!/usr/bin/env python3
"""P1 project lifecycle acceptance against a real macbotd.

The daemon and execution engine are real.  The only fake is the local
OpenAI-compatible provider inherited from ``smoke_dispatch``.  Each run uses
its own home and port, so this script is safe to run beside an installed
server.

The scenarios deliberately keep one coder assignment in a foreground bash
call, one tester assignment running, and a second coder assignment queued.
They then exercise the two project cancellation paths and verify the public
assignment list, the durable snapshot, trace history, and the project chat.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import time
import uuid

from smoke_collaboration import Daemon, http_json, rpc, wait_until
from smoke_dispatch import (
    DispatchProvider,
    assignments,
    assignment_for,
    settle_approvals,
    start_provider,
    traces,
    setup,
    wait_status,
)


TERMINAL = {"cancelled", "stopped", "failed", "done"}


def disk_assignments(home: Path) -> dict[str, dict]:
    state = json.loads((home / "data" / "orchestrator" / "state.json").read_text(encoding="utf-8"))
    return state.get("assignments", {})


def assert_canonical_event_seq(home: Path, base: str, password: str, ids: set[str]) -> None:
    """Ensure durable assignment events use the Store's single event cursor.

    The assignment list and snapshot are both projections of the same WAL.
    Check the event envelope cursor globally, then compare the latest event
    projection for each scenario assignment with both public and disk state.
    """
    events_path = home / "data" / "events" / "events.jsonl"
    assert events_path.exists(), events_path
    seqs: list[int] = []
    latest: dict[str, dict] = {}
    for line in events_path.read_text(encoding="utf-8").splitlines():
        event = json.loads(line)
        seq = event.get("seq")
        assert isinstance(seq, int), event
        seqs.append(seq)
        if event.get("event") not in {"assignment.created", "assignment.updated"}:
            continue
        assignment = event.get("data", {}).get("assignment")
        if not isinstance(assignment, dict) or assignment.get("id") not in ids:
            continue
        latest[assignment["id"]] = assignment
    assert seqs == sorted(set(seqs)), seqs
    listed = {item["id"]: item for item in assignments(base, password)}
    on_disk = disk_assignments(home)
    for assignment_id in ids:
        # A normally completed assignment can finish through the execution
        # path without an assignment.updated projection.  Cancellation must
        # always have a durable assignment event, since that is the mutation
        # under test and is what clients replay after reconnect.
        if assignment_id not in latest:
            assert listed.get(assignment_id, {}).get("status") == "done", assignment_id
            continue
        for source in (listed, on_disk):
            assert assignment_id in source, assignment_id
            for key in ("status", "project_id", "origin_chat_id", "finished_at"):
                assert latest[assignment_id].get(key) == source[assignment_id].get(key), (
                    assignment_id,
                    key,
                    latest[assignment_id].get(key),
                    source[assignment_id].get(key),
                )


def assert_rpc_disk_consistent(base: str, password: str, home: Path, ids: set[str]) -> None:
    listed = {item["id"]: item for item in assignments(base, password)}
    on_disk = disk_assignments(home)
    for assignment_id in ids:
        assert assignment_id in listed, assignment_id
        assert assignment_id in on_disk, assignment_id
        for key in ("status", "project_id", "origin_chat_id", "finished_at"):
            assert listed[assignment_id].get(key) == on_disk[assignment_id].get(key), (
                assignment_id,
                key,
                listed[assignment_id].get(key),
                on_disk[assignment_id].get(key),
            )
    assert_canonical_event_seq(home, base, password, ids)


def has_stop_system_message(messages: list[dict]) -> bool:
    """Match the durable project stop card on both current wire forms.

    The protocol represents a system message with ``sender.kind=system``;
    the current gateway also accepts the execution fallback card (main sender,
    blocked intent, and the stable Chinese text) while older binaries are
    being rolled forward.
    """
    for message in messages:
        sender = message.get("sender")
        sender_kind = sender.get("kind") if isinstance(sender, dict) else sender
        if sender_kind == "system":
            return True
        text = " ".join(
            str(message.get(key, ""))
            for key in ("text", "fallback_text")
        )
        blocks = message.get("blocks") or []
        if any(block.get("type") == "system" for block in blocks if isinstance(block, dict)):
            return True
        if message.get("intent") == "blocked" and "任务已停止" in text:
            return True
    return False


def project_assignments(base: str, password: str, project_id: str) -> list[dict]:
    return rpc(base, password, "assignment.list", {"project_id": project_id, "limit": 50})["items"]


def send_marker(base: str, password: str, chat_id: str, bot_id: str, marker: str, suffix: str) -> None:
    rpc(
        base,
        password,
        "chat.send",
        {
            "chat_id": chat_id,
            "text": marker,
            "mentions": [{"kind": "bot", "bot_id": bot_id, "instruction": marker}],
            # The same marker is intentionally reused in separate projects;
            # scope idempotency to the project chat so each scenario creates
            # its own assignment.
            "client_request_id": f"lifecycle:{chat_id}:{marker}:{suffix}",
        },
    )


def wait_project_status(base: str, password: str, project_id: str, status: str) -> dict:
    found: list[dict] = []

    def ready() -> bool:
        value = rpc(base, password, "project.get", {"project_id": project_id})["project"]
        found[:] = [value]
        return value.get("status") == status

    wait_until(ready, f"project {project_id} status {status}", 20)
    return found[0]


def wait_terminal(base: str, password: str, instruction: str) -> dict:
    return wait_status(base, password, instruction, TERMINAL, 30)


def start_bash_and_queue(
    base: str,
    password: str,
    project: dict,
    coder: dict,
    tester: dict,
    suffix: str,
    queue_marker: str,
    tester_marker: str,
) -> tuple[dict, dict, dict]:
    chat_id = project["chat"]["id"]
    send_marker(base, password, chat_id, coder["id"], "STOP_BASH", suffix)
    coder_working = wait_status(base, password, "STOP_BASH", {"working"}, 25)
    settle_approvals(base, password)
    wait_until(
        lambda: any(
            item.get("type") == "tool.start"
            and item.get("data", {}).get("name") == "bash"
            for item in traces(base, password, coder_working["id"])
        ),
        "foreground bash start",
        30,
    )
    send_marker(base, password, chat_id, tester["id"], tester_marker, suffix)
    tester_working = wait_status(base, password, tester_marker, {"working"}, 25)
    send_marker(base, password, chat_id, coder["id"], queue_marker, suffix)
    queued = wait_status(base, password, queue_marker, {"queued"}, 15)
    return coder_working, tester_working, queued


def assert_queued_never_started(base: str, password: str, queued: dict, marker: str) -> None:
    trace = traces(base, password, queued["id"])
    assert not any(item.get("type") == "run.start" for item in trace), trace
    assert marker not in {item.get("marker") for item in DispatchProvider.state.requests}


def remove_member_scenario(base: str, password: str, home: Path, coder: dict, tester: dict, suffix: str) -> dict:
    project = rpc(
        base,
        password,
        "project.create",
        {"name": f"remove-member-{suffix}", "goal": "cancel one member", "member_bot_ids": [coder["id"], tester["id"]]},
    )
    project_id = project["project"]["id"]
    coder_working, tester_working, queued = start_bash_and_queue(
        base, password, project, coder, tester, suffix, "REMOVE_QUEUED", "PAR_A"
    )
    response = rpc(
        base,
        password,
        "project.remove_member",
        {"project_id": project_id, "bot_id": coder["id"], "client_request_id": f"remove-member:{suffix}"},
    )
    assert coder["id"] not in {member["bot_id"] for member in response["project"]["members"]}
    removed = wait_terminal(base, password, "STOP_BASH")
    removed_queued = wait_terminal(base, password, "REMOVE_QUEUED")
    assert removed["status"] == "cancelled", removed
    assert removed_queued["status"] == "cancelled", removed_queued
    assert not Path(DispatchProvider.stop_path).exists(), "remove_member left a foreground bash sentinel"
    # A member removal must not cancel another Bot's work in the same group.
    tester_after = assignment_for(base, password, "PAR_A")
    assert tester_after["status"] != "cancelled", tester_after
    wait_until(lambda: assignment_for(base, password, "PAR_A")["status"] == "done", "remaining member completion", 30)
    assert_queued_never_started(base, password, removed_queued, "REMOVE_QUEUED")
    time.sleep(0.5)
    history = rpc(base, password, "chat.history", {"chat_id": project["chat"]["id"], "limit": 100})["messages"]
    assert has_stop_system_message(history), history
    ids = {coder_working["id"], tester_working["id"], queued["id"]}
    assert_rpc_disk_consistent(base, password, home, ids)
    return {
        "project_id": project_id,
        "removed": [removed["id"], removed_queued["id"]],
        "remaining": tester_after["id"],
        "chat_id": project["chat"]["id"],
    }


def confirm_done_scenario(base: str, password: str, home: Path, coder: dict, tester: dict, suffix: str) -> dict:
    project = rpc(
        base,
        password,
        "project.create",
        {"name": f"confirm-done-{suffix}", "goal": "cancel whole project", "member_bot_ids": [coder["id"], tester["id"]]},
    )
    project_id = project["project"]["id"]
    coder_working, tester_working, queued = start_bash_and_queue(
        base, password, project, coder, tester, suffix, "CONFIRM_QUEUED", "PAR_B"
    )
    done = rpc(
        base,
        password,
        "project.confirm_done",
        {"project_id": project_id, "client_request_id": f"confirm-done:{suffix}"},
    )["project"]
    assert done["status"] == "done", done
    for instruction in ("STOP_BASH", "PAR_B", "CONFIRM_QUEUED"):
        item = wait_terminal(base, password, instruction)
        assert item["status"] == "cancelled", item
    assert_queued_never_started(base, password, queued, "CONFIRM_QUEUED")
    assert not Path(DispatchProvider.stop_path).exists(), "confirm_done left a foreground bash sentinel"
    history = rpc(base, password, "chat.history", {"chat_id": project["chat"]["id"], "limit": 100})["messages"]
    assert has_stop_system_message(history), history
    ids = {coder_working["id"], tester_working["id"], queued["id"]}
    assert_rpc_disk_consistent(base, password, home, ids)
    return {"project_id": project_id, "assignments": sorted(ids), "chat_id": project["chat"]["id"]}


def confirm_review_scenario(base: str, password: str, home: Path, coder: dict, tester: dict, suffix: str) -> dict:
    project = rpc(
        base,
        password,
        "project.create",
        {"name": f"confirm-review-{suffix}", "goal": "review then cancel", "member_bot_ids": [coder["id"], tester["id"]]},
    )
    project_id = project["project"]["id"]
    coder_working, tester_working, _ = start_bash_and_queue(
        base, password, project, coder, tester, suffix, "REVIEW_QUEUED", "PAR_A"
    )
    reviewed = rpc(
        base,
        password,
        "project.request_review",
        {"project_id": project_id, "summary": "模型完成，等待用户确认", "client_request_id": f"review:{suffix}"},
    )
    assert reviewed["project"]["status"] == "review", reviewed
    done = rpc(
        base,
        password,
        "project.confirm_done",
        {"project_id": project_id, "client_request_id": f"review-confirm:{suffix}"},
    )["project"]
    assert done["status"] == "done", done
    statuses = {item["instruction"]: item["status"] for item in project_assignments(base, password, project_id)}
    assert any(status == "cancelled" for status in statuses.values()), statuses
    assert all(status in {"cancelled", "done"} for status in statuses.values()), statuses
    assert not Path(DispatchProvider.stop_path).exists(), "review confirm left a foreground bash sentinel"
    history = rpc(base, password, "chat.history", {"chat_id": project["chat"]["id"], "limit": 100})["messages"]
    assert has_stop_system_message(history), history
    assert_rpc_disk_consistent(
        base,
        password,
        home,
        {coder_working["id"], tester_working["id"]}
        | {item["id"] for item in project_assignments(base, password, project_id)},
    )
    return {"project_id": project_id, "statuses": statuses, "chat_id": project["chat"]["id"]}


def duplicate_section(base: str, password: str, home: Path, source: dict, suffix: str) -> dict:
    """Small independent regression for bot.duplicate's copy/rollback rules.

    Routines are copied with fresh IDs and no last_run.  Chat history is not
    copied into the new DM.  A disabled global skill stays disabled, and a
    conflicting duplicate name must leave the Bot/routine counts unchanged.
    """
    routine = rpc(
        base,
        password,
        "routine.create",
        {
            "bot_id": source["id"],
            "name": f"duplicate-routine-{suffix}",
            "instructions": f"duplicate-routine-marker-{suffix}",
            "schedules": [{"cron": "0 * * * *", "label": "hourly"}],
            "timezone": "Asia/Shanghai",
            "client_request_id": f"duplicate-routine:{suffix}",
        },
    )["routine"]
    history_marker = f"duplicate-history-{suffix}"
    rpc(
        base,
        password,
        "chat.send",
        {"chat_id": source["dm_chat_id"], "text": history_marker, "mentions": [], "client_request_id": f"duplicate-chat:{suffix}"},
    )
    skill_name = f"duplicate-disabled-{suffix}"
    rpc(
        base,
        password,
        "skill.create",
        {"name": skill_name, "content": f"---\nname: {skill_name}\ndescription: duplicate regression\n---\nDISABLED", "client_request_id": f"duplicate-skill:{suffix}"},
    )
    rpc(base, password, "skill.set_enabled", {"name": skill_name, "enabled": False, "client_request_id": f"duplicate-skill-off:{suffix}"})
    before = rpc(base, password, "bot.list", {"include_hidden": True})["bots"]
    duplicate = rpc(base, password, "bot.duplicate", {"bot_id": source["id"], "name": f"duplicate-{suffix}"})
    copied = duplicate["bot"]
    assert copied["id"] != source["id"]
    assert copied["dm_chat_id"] == duplicate["dm_chat"]["id"]
    for key in ("model", "tools", "browser_mode", "max_parallel"):
        assert copied.get(key) == source.get(key), (key, copied, source)
    source_routines = rpc(base, password, "routine.list", {"bot_id": source["id"]})["routines"]
    copied_routines = rpc(base, password, "routine.list", {"bot_id": copied["id"]})["routines"]
    assert any(item["id"] == routine["id"] for item in source_routines)
    assert len(copied_routines) == len(source_routines)
    assert {item["id"] for item in copied_routines}.isdisjoint({item["id"] for item in source_routines})
    assert all(item.get("last_run") is None for item in copied_routines), copied_routines
    source_history = rpc(base, password, "chat.history", {"chat_id": source["dm_chat_id"], "limit": 100})["messages"]
    copied_history = rpc(base, password, "chat.history", {"chat_id": copied["dm_chat_id"], "limit": 100})["messages"]
    assert any(history_marker in item.get("fallback_text", "") for item in source_history), source_history
    assert not any(history_marker in item.get("fallback_text", "") for item in copied_history), copied_history
    assert rpc(base, password, "skill.get", {"name": skill_name})["skill"]["enabled"] is False

    # Duplicate-name failure must be atomic: no extra Bot or copied routine.
    before_ids = {item["id"] for item in before} | {source["id"]}
    failed = http_json(
        f"{base}/api/v1/rpc",
        {"method": "bot.duplicate", "params": {"bot_id": source["id"], "name": f"duplicate-{suffix}"}},
        password,
    )
    assert failed.get("ok") is False, failed
    after = rpc(base, password, "bot.list", {"include_hidden": True})["bots"]
    assert {item["id"] for item in after} == before_ids | {copied["id"]}, (before_ids, after)
    assert len(rpc(base, password, "routine.list", {"bot_id": copied["id"]})["routines"]) == len(copied_routines)
    return {"source_bot": source["id"], "duplicate_bot": copied["id"], "routine": routine["id"], "history_isolated": True, "rollback": True}


def acceptance(args: argparse.Namespace) -> None:
    base = args.url.rstrip("/")
    provider, provider_url = start_provider()
    daemon = Daemon(args)
    DispatchProvider.state = type(DispatchProvider.state)()
    try:
        daemon.start()
        suffix = uuid.uuid4().hex[:8]
        _model, coder, tester, _provider = setup(base, args.password, provider_url, suffix)
        DispatchProvider.coder_id = coder["id"]
        DispatchProvider.tester_id = tester["id"]
        DispatchProvider.stop_path = str(args.home / "lifecycle-stop-sentinel")
        remove = remove_member_scenario(base, args.password, args.home, coder, tester, suffix)
        confirm = confirm_done_scenario(base, args.password, args.home, coder, tester, suffix)
        review = confirm_review_scenario(base, args.password, args.home, coder, tester, suffix)
        duplicate = duplicate_section(base, args.password, args.home, coder, suffix) if args.with_duplicate else None
        assert not Path(DispatchProvider.stop_path).exists()
        print(json.dumps({"ok": True, "remove_member": remove, "confirm_done": confirm, "confirm_review": review, "duplicate": duplicate, "home": str(args.home)}, ensure_ascii=False))
    finally:
        daemon.close()
        provider.shutdown()
        provider.server_close()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default="http://127.0.0.1:7794")
    parser.add_argument("--password", default="dev")
    parser.add_argument("--daemon-command")
    parser.add_argument("--home", type=Path, default=Path("/tmp/macbot-lifecycle-smoke"))
    parser.add_argument("--browser-bin")
    parser.add_argument("--with-duplicate", action="store_true", help="also run the independent bot.duplicate regression")
    args = parser.parse_args()
    if args.home.exists():
        shutil.rmtree(args.home)
    acceptance(args)


if __name__ == "__main__":
    main()
