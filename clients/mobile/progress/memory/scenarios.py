#!/usr/bin/env python3
"""Verification-only WebSocket relay for the populated-profile screenshot.

All RPCs and events pass through to the isolated mock unchanged except the
response to memory.profile.get, where the relay sets only profile.onboarded to
true. This validates client rendering of an initialized profile; it does not
exercise the server's conversational onboarding flow.
"""

import argparse
import asyncio
import json

from websockets.asyncio.client import connect
from websockets.asyncio.server import serve


async def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen", default="127.0.0.1", help="proxy bind address")
    parser.add_argument("--port", type=int, default=7790, help="proxy port")
    parser.add_argument(
        "--upstream", default="ws://127.0.0.1:7789", help="isolated mock base URL"
    )
    args = parser.parse_args()

    async def handle(client):
        path = client.request.path
        if not path.startswith("/ws?") and path != "/ws":
            await client.close(code=1008, reason="only /ws is supported")
            return
        async with connect(f"{args.upstream}{path}") as mock:
            profile_request_ids = set()

            async def to_mock():
                async for frame in client:
                    if isinstance(frame, str):
                        try:
                            request = json.loads(frame)
                            if request.get("method") == "memory.profile.get":
                                profile_request_ids.add(request.get("id"))
                        except (json.JSONDecodeError, AttributeError):
                            pass
                    await mock.send(frame)

            async def to_client():
                async for frame in mock:
                    if isinstance(frame, str):
                        try:
                            response = json.loads(frame)
                            request_id = response.get("id")
                            result = response.get("result")
                            profile = result.get("profile") if isinstance(result, dict) else None
                            if (
                                request_id in profile_request_ids
                                and isinstance(profile, dict)
                            ):
                                profile["onboarded"] = True
                                frame = json.dumps(response, ensure_ascii=False)
                                profile_request_ids.discard(request_id)
                                print("fixture: profile.get onboarded=true", flush=True)
                        except (json.JSONDecodeError, AttributeError, TypeError):
                            pass
                    await client.send(frame)

            tasks = {
                asyncio.create_task(to_mock()),
                asyncio.create_task(to_client()),
            }
            _, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in pending:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)

    async with serve(handle, args.listen, args.port, max_size=None):
        print(
            f"profile fixture relay listening on {args.listen}:{args.port}; "
            f"upstream {args.upstream}",
            flush=True,
        )
        await asyncio.Future()


if __name__ == "__main__":
    asyncio.run(main())
