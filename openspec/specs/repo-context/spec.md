# repo-context Specification

## Purpose
What an agent phase is told about the repository up front, sized for small context windows: a bounded gitignore-aware tree, a ranked symbol map, scoped prior artifacts, and in-loop token budgeting.
## Requirements
### Requirement: Bounded repository tree

The system SHALL inject a repository tree into agent prompts that is
gitignore-aware, depth-bounded, and size-capped, and that — when the repository
exceeds the cap — degrades gracefully by summarizing (e.g. directory counts)
rather than emitting a truncated flat list, and points the agent at
`search_code` / `list_files` to explore further.

#### Scenario: Small repository

- **WHEN** the repository fits within the size cap
- **THEN** the injected tree shows its structure within the depth bound.

#### Scenario: Large repository

- **WHEN** the repository exceeds the size cap
- **THEN** the tree is summarized within the cap and directs the agent to use `search_code` / `list_files`, rather than emitting a truncated flat list of entries.

#### Scenario: Ignored paths excluded

- **WHEN** the repository contains gitignored paths (e.g. `target/`, `node_modules/`)
- **THEN** those paths are excluded from the injected tree.

### Requirement: Ranked symbol repo map

The system SHALL be able to produce a repo map: for source files, the key
symbols they define (function / type / class signatures), ranked by relevance to
the run's idea, bounded to a token budget, and made available to prompts as a
context variable. The map SHALL degrade to fewer files/symbols when the budget
is tight rather than exceeding it. Relevance SHALL be the BM25 score of the idea
against the file's path and full source text, so words that appear only in
bodies or comments count, and words common across the repository count less than
rare ones.

Among files of similar relevance, the map SHALL favor recently changed files. In
a git repository, recency SHALL come from git: files with uncommitted changes
first, then files by how recently a commit touched them. Git SHALL be run inside
the shell sandbox without network, so repository-controlled git configuration
cannot execute commands on the host. When git history is unavailable, recency
SHALL fall back to file modification times.

#### Scenario: Map lists ranked symbols

- **WHEN** a repo map is built for an idea
- **THEN** it lists source files with their key symbol signatures, ordered by relevance to the idea.

#### Scenario: Body terms count, common words do not dominate

- **WHEN** only one file mentions a rare idea term, in a function body, and other files repeat the idea's common words
- **THEN** that file ranks first.

#### Scenario: Map respects a token budget

- **WHEN** the full symbol set would exceed the map's token budget
- **THEN** the map includes the highest-ranked files/symbols within the budget and indicates it is partial.

#### Scenario: Recently committed file breaks a tie

- **WHEN** two files are equally relevant to the idea and one was touched by a more recent commit
- **THEN** the more recently committed file ranks first, regardless of file modification times.

#### Scenario: Uncommitted changes rank as most recent

- **WHEN** a file has uncommitted changes
- **THEN** it receives the highest recency boost.

#### Scenario: Not a git repository

- **WHEN** the repository has no git history (or git or the sandbox is unavailable)
- **THEN** recency falls back to file modification times and the map is still built.

### Requirement: Per-phase context scoping

The system SHALL give each phase only the prior-artifact context it needs, and
SHALL NOT inject both the full `prior_artifacts` map and each artifact key
redundantly. Prompt-visible context for a phase SHALL be derived from that
phase's declared inputs / template, not the entire accumulated history by
default.

#### Scenario: No redundant artifact injection

- **WHEN** a phase prompt is assembled
- **THEN** prior artifacts are exposed once (not duplicated as both a full map and individual keys).

#### Scenario: Phase sees relevant prior work

- **WHEN** a phase renders its prompt
- **THEN** it can access the prior artifacts its template references without the full unrelated history being force-injected.

### Requirement: Context-window budgeting in the agent loop

The system SHALL track the running input-token size of the agent conversation
and, when it approaches a configured fraction of the model's context window,
reduce older tool results (truncate or summarize) so the request stays within
the window. Budgeting SHALL preserve the system prompt, the task, and the most
recent turns.

#### Scenario: Trim when approaching the window

- **WHEN** accumulated input tokens exceed the configured fraction of the context window
- **THEN** older tool results are truncated/summarized so the next request fits, while the system prompt, task, and latest turns are preserved.

#### Scenario: Unknown window falls back safely

- **WHEN** the model's context window is unknown (e.g. a local model)
- **THEN** budgeting uses a configured default budget rather than allowing unbounded growth.

### Requirement: Fallback prompt carries phase context

When a phase has no prompt file, the generic prompt it receives SHALL include the content of each prior artifact the phase may see, the repo map and pushed context when those are enabled, and the tool list when tools are enabled. The repository tree SHALL mark directories with a trailing `/` and never a leading one.

#### Scenario: Implement sees the plan

- **WHEN** a template without prompt files runs `plan` then `implement`
- **THEN** the implement prompt contains the plan artifact's content.

#### Scenario: Context flags reach a custom template

- **WHEN** `context.repo_map` or retrieval is on for a phase without a prompt file
- **THEN** the repo map and retrieved code appear in its prompt.

#### Scenario: Tree paths are usable

- **WHEN** the repository has a `src` directory
- **THEN** the tree shows `src/`, not `/src`.

