# Android reply association check

- `ChatScreen.kt:596` assigns the swiped message ID to `replyTo`; `ChatScreen.kt:770` submits it as `quote` with `quote_mode=reply`.
- `docs/PROTOCOL.md:224-225, 576` specifies that `Message.reply_to` is always null and the quote relationship is `Message.quote.message_id`; `reply_to_message_id` is not a protocol field.
- The batch-01 note only checked the deprecated `reply_to` and nonexistent `reply_to_message_id`, so its `reply` fail result is not supported by that RPC observation alone.
- Independent RPC reproduction in `rpc-evidence.json`: `chat_login` original at seq 18 is `msg_mock_login_questions`; sending the matching `chat.send` payload produced seq 19 whose `quote.message_id` is `msg_mock_login_questions` in both response and subsequent `chat.history`.
- No client code changed. To close the original batch-01 assertion, compare its seq-19 `quote.message_id` with the actual swiped source message ID; that original full RPC payload was not retained. Update the acceptance note to test `quote.message_id` and classify the old observation as inconclusive until the original evidence can be re-read.
