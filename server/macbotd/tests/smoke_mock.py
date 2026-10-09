#!/usr/bin/env python3
"""Live S0 wire acceptance; start macbotd --mock with an isolated data directory.

python -m pip install websockets jsonschema
python server/macbotd/tests/smoke_mock.py --url http://127.0.0.1:7789
"""
import argparse
import asyncio
import json
from pathlib import Path
import struct
import urllib.error
import urllib.request
import uuid

import jsonschema
import websockets

REPO = Path(__file__).resolve().parents[3]
SCHEMAS = REPO / "protocol/schema"
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def typed(name, value):
    jsonschema.validate(value, json.loads((SCHEMAS / f"{name}.json").read_text()))


def rpc(url, password, method, params=None):
    request = urllib.request.Request(
        url + "/api/v1/rpc",
        json.dumps({"method": method, "params": params or {}}).encode(),
        {"Content-Type": "application/json", "Authorization": f"Bearer {password}"},
    )
    with opener.open(request, timeout=15) as response:
        body = json.load(response)
    assert body["ok"], (method, body.get("error"))
    return body["result"]


async def receive(ws, predicate, timeout=10):
    async def matching():
        while True:
            raw = await ws.recv()
            assert isinstance(raw, str), "main channel must contain only text"
            value = json.loads(raw)
            if predicate(value):
                return value
    return await asyncio.wait_for(matching(), timeout)


async def request(ws, method, params):
    request_id = str(uuid.uuid4())
    await ws.send(json.dumps({"v": 1, "kind": "req", "id": request_id, "method": method, "params": params}))
    frame = await receive(ws, lambda x: x.get("kind") == "res" and x.get("id") == request_id)
    assert frame["ok"], (method, frame.get("error"))
    return frame["result"]


def parse_screen(raw):
    assert isinstance(raw, bytes)
    size = struct.unpack(">I", raw[:4])[0]
    header = json.loads(raw[4:4 + size])
    typed("screen_frame_header", header)
    jpeg = raw[4 + size:]
    assert jpeg.startswith(b"\xff\xd8") and jpeg.endswith(b"\xff\xd9")
    assert header["w"] > 1 and header["h"] > 1
    return header, jpeg


async def acceptance(url, password):
    health = json.load(opener.open(url + "/api/v1/health"))
    assert health["ok"] and health["protocol"] == 1 and not health["setup_required"]
    try:
        opener.open(urllib.request.Request(url + "/api/v1/rpc", b'{"method":"bootstrap","params":{}}', {"Content-Type": "application/json"}))
        raise AssertionError("unauthenticated RPC accepted")
    except urllib.error.HTTPError as error:
        assert error.code == 401 and json.load(error)["error"] == "unauthorized"
    bootstrap = rpc(url, password, "bootstrap")
    typed("hello", bootstrap["hello"])
    typed("settings", bootstrap["settings"])
    for collection, name in [("bots", "bot"), ("chats", "chat"), ("projects", "project")]:
        for item in bootstrap[collection]:
            typed(name, item)
    main = next(bot for bot in bootstrap["bots"] if bot["is_main"])
    chat_id = main["dm_chat_id"]
    client_request_id = str(uuid.uuid4())
    params = {"chat_id": chat_id, "text": "S0 wire acceptance", "mentions": [], "client_request_id": client_request_id}
    first = rpc(url, password, "chat.send", params)
    second = rpc(url, password, "chat.send", params)
    assert first == second, "write replay must return the first result"
    typed("message", first["message"])
    bot = rpc(url, password, "bot.create", {"name": "wire-check", "client_request_id": str(uuid.uuid4())})
    typed("bot", bot["bot"])
    typed("chat", bot["dm_chat"])
    project = rpc(url, password, "project.create", {"name": "wire-check", "goal": "S0", "member_bot_ids": [bot["bot"]["id"]], "client_request_id": str(uuid.uuid4())})
    typed("project", project["project"])
    typed("chat", project["chat"])
    project_get = rpc(url, password, "project.get", {"project_id": project["project"]["id"]})
    typed("project", project_get["project"])
    typed("announcement", project_get["announcement"])
    typed("project", rpc(url, password, "project.confirm_done", {"project_id": project["project"]["id"]})["project"])
    ws_url = url.replace("http://", "ws://").replace("https://", "wss://")
    async with websockets.connect(ws_url + "/ws", additional_headers={"Authorization": f"Bearer {password}"}, proxy=None) as listener:
        hello = json.loads(await listener.recv())
        assert hello["event"] == "hello" and "seq" not in hello
        typed("hello", hello["data"])
        assert hello["data"]["node_id"] == bootstrap["hello"]["node_id"], "Host identity differs across transports"
        result = await request(listener, "session.resume", {"last_seq": 0, "client": {"platform": "macos", "app_version": "test", "device_name": "wire", "device_id": "wire"}})
        assert result["mode"] == "reset"
        await receive(listener, lambda x: x.get("event") == "sync.done")
        current = await request(listener, "bootstrap", {})
        assignments = await request(listener, "assignment.list", {})
        traced = None
        for assignment in assignments["items"]:
            history = await request(listener, "trace.history", {"assignment_id": assignment["id"], "tail": True, "limit": 500})
            if history["items"]:
                traced = (assignment, history["items"])
                break
        assert traced, "canonical collaboration scenario must expose runtime traces"
        assignment, items = traced
        for item in items:
            typed("trace_item", item)
        since = items[len(items) // 2]["aseq"]
        subscribed = await request(listener, "trace.subscribe", {"assignment_id": assignment["id"], "since_aseq": since})
        expected = [item for item in items if item["aseq"] > since]
        for item in expected:
            event = await receive(listener, lambda x: x.get("event") == "trace.item" and x["data"]["stream"] == subscribed["stream"])
            assert event["data"]["item"] == item, "trace cursor replay differs from history"
        await request(listener, "trace.unsubscribe", {"stream": subscribed["stream"]})
        cursor = current["seq"]
        sent = await asyncio.to_thread(rpc, url, password, "chat.send", {"chat_id": chat_id, "text": "live event", "mentions": []})
        live = await receive(listener, lambda x: x.get("event") == "message.created" and x["data"]["message"]["id"] == sent["message"]["id"])
        assert live["seq"] > cursor
        typed("event_frame", live)
    async with websockets.connect(ws_url + "/ws", additional_headers={"Authorization": f"Bearer {password}"}, proxy=None) as reconnected:
        await reconnected.recv()
        result = await request(reconnected, "session.resume", {"last_seq": cursor, "client": {"platform": "macos", "app_version": "test", "device_name": "wire", "device_id": "wire"}})
        assert result["mode"] == "replay"
        replay = await receive(reconnected, lambda x: x.get("event") == "message.created" and x["data"]["message"]["id"] == sent["message"]["id"])
        assert replay["seq"] == live["seq"]
    async with websockets.connect(ws_url + f"/ws/screen?bot_id={main['id']}", additional_headers={"Authorization": f"Bearer {password}"}, proxy=None) as screen:
        state = json.loads(await screen.recv())
        assert state["type"] == "state"
        typed("screen_state", state["state"])
        head, first_jpeg = parse_screen(await screen.recv())
        try:
            await asyncio.wait_for(screen.recv(), .3)
            raise AssertionError("screen sent an unacknowledged second frame")
        except asyncio.TimeoutError:
            pass
        await screen.send(json.dumps({"type": "ack", "seq": head["seq"] - 1}))
        try:
            await asyncio.wait_for(screen.recv(), .1)
            raise AssertionError("stale ACK released a frame")
        except asyncio.TimeoutError:
            pass
        await screen.send(json.dumps({"type": "ack", "seq": head["seq"]}))
        next_head, next_jpeg = parse_screen(await screen.recv())
        assert next_head["seq"] > head["seq"] and next_jpeg != first_jpeg
    print("S0 live acceptance passed: typed RPC, Host identity, idempotency, auth, live events, replay, trace cursor, JPEG ACK throttle")


if __name__ == "__main__":
    args = argparse.ArgumentParser()
    args.add_argument("--url", default="http://127.0.0.1:7789")
    args.add_argument("--password", default="dev")
    parsed = args.parse_args()
    asyncio.run(acceptance(parsed.url.rstrip("/"), parsed.password))
