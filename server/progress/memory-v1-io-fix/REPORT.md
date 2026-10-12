# 持续记忆 I/O 错误验收

候选修复通过独立验证。目录注入从 memory 工具获批前持续保留到关联 `run.end`；本地确定性 OpenAI-compatible provider 在第二次 HTTP 请求中实际收到原 memory 写入错误，以及 `snapshot_cache_put` 合成的 `role=tool, is_error=true` 结果。run 以 `done` 结束。

- `cargo test -p macbot-memory -p macbot-gateway`：341 passed（gateway 324、memory 17），0 failed。
- `cargo clippy -p macbot-memory -p macbot-gateway -p macbot-providers -- -D warnings`：通过。
- 回归测试：`persistent_snapshot_write_failure_is_returned_and_persisted`、`context_write_failure_adds_a_matched_error_tool_result`：各 1 passed。
- 端到端：`python3 server/progress/memory-v1-io-fix/e2e_memory_io_failure.py --evidence /tmp/macbot-memory-io-fix-e2e.json`：PASS；4 次 provider 请求；第 2 次请求携带两类错误工具结果；同一 run 的原始工具错误和快照失败均有 `memory.failed` WAL 记录；WAL seq 与失败 call ID 唯一；注入目录在 `run.end` 时仍存在。守护进程、provider 与临时 home 均由 harness 清理。
- 原始 provider 请求、trace、WAL 和检查结果：`/tmp/macbot-memory-io-fix-e2e.json`（不含真实密钥）。
