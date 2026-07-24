# LibreJustice — agent install guide

This guide is written for AI agents (Cline, Claude Code, Codex, Cursor,
Windsurf…) installing LibreJustice: a remote MCP server for searching
French and European case law and statutes, plus two method skills.

## 1. Connect the MCP server

- Endpoint: `https://librejustice.fr/mcp`
- Transport: streamable HTTP
- Auth: OAuth 2.1, auto-discovered by the client. The browser flow
  creates a free account if the user does not have one.

Claude Code — prefer the plugin, it installs the connector and the
skills together (then skip step 2):

```
/plugin marketplace add librejustice/librejustice
/plugin install librejustice@librejustice
```

or connector only:

```
claude mcp add --transport http librejustice https://librejustice.fr/mcp
```

Codex CLI:

```
codex plugin marketplace add librejustice/librejustice
codex plugin add librejustice@librejustice
```

Any client that reads a JSON MCP config — add to `mcpServers`:

```json
{"librejustice": {"type": "http", "url": "https://librejustice.fr/mcp"}}
```

## 2. Install the skills

The two skills teach the research method (read decisions in full
before citing, fetch statutes as of the relevant date, corpus traps).
The connector works without them, but answers are markedly more
reliable with them.

```
npx skills add librejustice/librejustice
```

or copy the `skills/recherche-jurisprudence` and
`skills/recherche-normes` folders of this repository into the agent's
skills directory.

## 3. Verify

Call the `search_decisions` tool with the query « infection nosocomiale
responsabilité » — it must return results. If the client asks for OAuth
authorization, open the flow and wait for the user to approve it.

## If you cannot install

If you can neither run commands nor edit your own configuration, say
so and point the user to <https://librejustice.fr/mcp-guide> — it has
step-by-step instructions per client (claude.ai, ChatGPT, Le Chat,
Perplexity, agent CLIs).
