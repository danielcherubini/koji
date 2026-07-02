# Use MADR for architectural decisions

## Context and Problem Statement

Tama makes architectural decisions — backend selection, WASM vs. SSR, database schema, etc. — that need to be recorded with their rationale so future contributors understand the "why" behind them. Ad-hoc documentation in READMEs or commit messages scatters context and makes it hard to trace decisions.

## Decision Drivers

* Decisions should be discoverable and searchable
* Minimal overhead to write and review
* Version-controlled alongside code
* Plain text format, no proprietary tooling

## Considered Options

* MADR (Markdown Architectural Decision Records)
* Michael Nygard's original ADR template
* Structured MADR (YAML frontmatter extension)
* No formal ADR process

## Decision Outcome

Chosen option: "MADR", because it is the most widely adopted Markdown-based ADR format, has a minimal template for quick decisions and a full template for complex ones, and requires no external tooling.

### Consequences

* Good, because decisions live in `docs/decisions/` as plain Markdown — easy to grep, review in PRs, and navigate in any editor
* Good, because the sequential numbering gives a chronological record of decisions
* Bad, because nothing enforces writing ADRs — discipline is required to keep them current
* Bad, because MADR has no built-in status tracking (e.g. superseded) beyond a convention in the title or frontmatter

## More Information

* [MADR homepage](https://adr.github.io/madr/)
* [MADR GitHub repository](https://github.com/adr/madr)
* [MADR 4.0.0 release](https://github.com/adr/madr/releases/tag/4.0.0)
* Scientific paper: [MADR: Format and Tool Support](https://dblp.org/rec/conf/zeus/KoppAZ18.html)
