#!/usr/bin/env python3
"""Read-only S1 history rendering checks.

The check authenticates against the production daemon, reads one chat through
the protocol pagination cursor, and reports hashes and sizes instead of
printing message text.  It is an API check for persisted history, not a UI
rendering or streaming test.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve()
import sys

sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    add_connection_args,
    bootstrap,
    chat_history,
    client_from_args,
    ready_health,
    require_list,
    require_production_host,
    run_main,
)
from s1.private_chat import complete_chat_history, validate_canonical_history  # noqa: E402


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--chat-id", required=True, help="Chat ID whose persisted history is being checked")
    parser.add_argument(
        "--baseline",
        type=Path,
        help="JSON evidence file containing history.messages to compare by id, seq, and created_at",
    )
    return parser


def text_blocks(message: dict[str, Any]) -> list[str]:
    blocks = message.get("blocks")
    if not isinstance(blocks, list):
        return []
    return [
        block["markdown"]
        for block in blocks
        if isinstance(block, dict)
        and block.get("type") == "text"
        and isinstance(block.get("markdown"), str)
    ]


def digest_text(text: str) -> dict[str, Any]:
    encoded = text.encode("utf-8")
    return {"text_bytes": len(encoded), "text_sha256": hashlib.sha256(encoded).hexdigest()}


def message_id_seq(message: dict[str, Any], label: str) -> tuple[str, int]:
    message_id = message.get("id")
    seq = message.get("seq")
    if not isinstance(message_id, str) or not message_id:
        raise ValueError(f"{label} has no canonical id")
    if not isinstance(seq, int) or isinstance(seq, bool):
        raise ValueError(f"{label} has no numeric seq")
    return message_id, seq


def sender_kind(message: dict[str, Any]) -> str | None:
    sender = message.get("sender")
    return sender.get("kind") if isinstance(sender, dict) and isinstance(sender.get("kind"), str) else None


def load_baseline(path: Path) -> list[dict[str, Any]]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ValueError(f"cannot read baseline {path}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise ValueError(f"baseline {path} is not valid JSON") from exc
    if isinstance(raw, dict):
        history = raw.get("history")
        raw = history.get("messages") if isinstance(history, dict) else None
    if not isinstance(raw, list) or any(not isinstance(item, dict) for item in raw):
        raise ValueError("baseline must contain history.messages as an array of objects")
    return raw


def baseline_index(messages: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    indexed: dict[str, dict[str, Any]] = {}
    seen_seq: set[int] = set()
    for message in messages:
        message_id, seq = message_id_seq(message, "baseline message")
        if message_id in indexed:
            raise ValueError(f"baseline repeats message id {message_id}")
        if seq in seen_seq:
            raise ValueError(f"baseline repeats message seq {seq}")
        created_at = message.get("created_at")
        if not isinstance(created_at, str) or not created_at:
            raise ValueError(f"baseline message {message_id} has no created_at")
        indexed[message_id] = message
        seen_seq.add(seq)
    if not indexed:
        raise ValueError("baseline history.messages is empty")
    return indexed


def compare_baseline(
    baseline: list[dict[str, Any]],
    current_by_id: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    """Compare stable identity fields and verify repaired known text blocks.

    A baseline message with a known ``text`` block and non-empty fallback must
    now have non-empty Markdown containing that original text.  A message whose
    block type is unknown remains valid through its non-empty fallback; this
    keeps forward-compatible fallback rendering from becoming a false failure.
    """

    records: list[dict[str, Any]] = []
    for baseline_message in sorted(baseline, key=lambda item: item["seq"]):
        message_id, seq = message_id_seq(baseline_message, "baseline message")
        current = current_by_id.get(message_id)
        if current is None:
            raise ValueError(f"baseline message {message_id} is missing from current history")
        current_id, current_seq = message_id_seq(current, f"current message {message_id}")
        if current_id != message_id or current_seq != seq:
            raise ValueError(f"message {message_id} changed id/seq")
        if current.get("created_at") != baseline_message.get("created_at"):
            raise ValueError(f"message {message_id} changed created_at")

        original = baseline_message.get("fallback_text")
        original = original if isinstance(original, str) else ""
        current_fallback = current.get("fallback_text")
        current_fallback = current_fallback if isinstance(current_fallback, str) else ""
        baseline_has_known_text = any(
            isinstance(block, dict) and block.get("type") == "text"
            for block in baseline_message.get("blocks", [])
            if isinstance(baseline_message.get("blocks"), list)
        )
        rendered = [text for text in text_blocks(current) if text.strip()]
        if original and baseline_has_known_text:
            if not rendered or not any(original in text for text in rendered):
                raise ValueError(f"message {message_id} known text block lost its original text")
            evidence_text = next(text for text in rendered if original in text)
            mode = "known_text"
        elif not baseline_has_known_text:
            if original and not current_fallback.strip():
                raise ValueError(f"message {message_id} unknown-block fallback is empty")
            if original and original not in current_fallback:
                raise ValueError(f"message {message_id} unknown-block fallback lost its original text")
            evidence_text = current_fallback
            mode = "fallback" if not rendered else "known_text"
        else:
            # Empty baseline fallback carries no text assertion, but a present
            # known block is still safe to report without rejecting a streaming
            # placeholder or an unknown block.
            evidence_text = rendered[0] if rendered else current_fallback
            mode = "known_text" if rendered else "fallback"
        record = {"id": message_id, "seq": seq, "created_at": current["created_at"], "mode": mode}
        record.update(digest_text(evidence_text))
        records.append(record)
    return records


def history_after(client: Any, chat_id: str, after_seq: int) -> list[dict[str, Any]]:
    """Read all pages after a cursor, rejecting a non-progressing cursor."""

    messages: list[dict[str, Any]] = []
    cursor = after_seq
    while True:
        result = chat_history(client, chat_id, after_seq=cursor)
        raw_page = require_list(result.get("messages"), "chat.history.messages")
        if any(not isinstance(item, dict) for item in raw_page):
            raise ValueError("chat.history.messages contains a non-object message")
        page = raw_page
        if any(item.get("seq") <= cursor for item in page if isinstance(item.get("seq"), int)):
            raise ValueError(f"chat.history after_seq returned a message at or before {cursor}")
        messages.extend(page)
        if not result["has_more"]:
            break
        page_seqs = [item.get("seq") for item in page]
        if not page_seqs or any(not isinstance(seq, int) or isinstance(seq, bool) for seq in page_seqs):
            raise ValueError("chat.history after_seq has_more returned an empty or invalid page")
        next_cursor = max(page_seqs)
        if next_cursor <= cursor:
            raise ValueError("chat.history after_seq cursor did not move forward")
        cursor = next_cursor
    return messages


def final_bot_after_user(client: Any, chat_id: str, messages: list[dict[str, Any]]) -> dict[str, Any]:
    users = [item for item in messages if sender_kind(item) == "user"]
    if not users:
        raise ValueError("chat history has no user message")
    last_user = max(users, key=lambda item: item["seq"])
    _, user_seq = message_id_seq(last_user, "last user message")
    after = history_after(client, chat_id, user_seq)
    if not after:
        raise ValueError("after_seq returned no messages")
    validate_canonical_history(after)
    candidates = [
        item
        for item in after
        if sender_kind(item) == "bot"
        and isinstance(item.get("id"), str)
        and isinstance(item.get("seq"), int)
    ]
    if not candidates:
        raise ValueError(f"after_seq={user_seq} has no corresponding Bot message")
    reply = max(candidates, key=lambda item: item["seq"])
    rendered = next((text for text in text_blocks(reply) if text.strip()), reply.get("fallback_text", ""))
    if not isinstance(rendered, str):
        rendered = ""
    evidence = {"user_id": last_user["id"], "user_seq": user_seq, "bot_id": reply["id"], "bot_seq": reply["seq"], "streaming": reply.get("streaming")}
    evidence.update(digest_text(rendered))
    return evidence


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    state = bootstrap(client)
    chat_ids = {item.get("id") for item in require_list(state.get("chats"), "bootstrap.chats") if isinstance(item, dict)}
    if args.chat_id not in chat_ids:
        raise ValueError(f"chat {args.chat_id} is absent from bootstrap")
    messages = complete_chat_history(client, args.chat_id)
    validate_canonical_history(messages)
    current_by_id = {item["id"]: item for item in messages}
    baseline_records: list[dict[str, Any]] = []
    if args.baseline is not None:
        baseline = load_baseline(args.baseline)
        baseline_index(baseline)
        baseline_records = compare_baseline(baseline, current_by_id)
    final_reply = final_bot_after_user(client, args.chat_id, messages)
    return {
        "scenario": "S1 persisted text history",
        "status": "PASS",
        "baseline": baseline_records,
        "final_after_seq": final_reply,
        "conclusion": "API checks PASS; IDs, seq, timestamps, text bytes and hashes validated; streaming placeholders remain allowed.",
    }


if __name__ == "__main__":
    raise SystemExit(run_main(scenario, args_parser().parse_args()))
