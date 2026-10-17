## MODIFIED Requirements

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
