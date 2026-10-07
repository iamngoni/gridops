# Third-party notices

## Heimdall provider connection flow

`crates/gridops-agent/src/provider_auth.rs` adapts the ChatGPT (Codex) and
Claude Code OAuth connection flows, and `crates/gridops-agent/src/subscriptions.rs`
adapts the matching model transports, from [Codecraft Solutions ZA's Heimdall project](https://github.com/iamngoni/heimdall)
by way of ccs. Those adapted modules keep their copyright and SPDX notices and
are governed by the [Functional Source License 1.1 with MIT Future License](https://github.com/iamngoni/heimdall/blob/master/LICENSE),
not the MIT License that covers the rest of GridOps.

## Agent Runtime

`crates/gridops-agent` uses [iamngoni/agent-runtime](https://github.com/iamngoni/agent-runtime),
pinned to a reviewed commit, for provider-neutral messages, tool definitions,
and the OpenAI-compatible and Anthropic clients. Agent Runtime is licensed under
the MIT License.
