# ADR 001: WebSocket multiplexed RPC over REST+SSE

**Status:** Accepted

**Context:** Bridge must stream watch events and serve request/response for history/chats.

**Decision:** Single WSS connection with typed envelopes (`req`/`res`/`event`).

**Consequences:** One TLS session, simpler reconnect, explicit method allowlist at bridge layer.
