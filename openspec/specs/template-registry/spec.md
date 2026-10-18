# template-registry Specification

## Purpose
TBD - created by archiving change template-registry. Update Purpose after archive.
## Requirements
### Requirement: Install a pinned template from git

The system SHALL install a template from a git repository given as `<url>[//<path>][@<ref>]`, resolving the ref (default: the remote's default branch) to a commit SHA and recording the source, path, ref, commit, file hashes and trust state in `~/.flows/templates/registry.lock`. `<path>` SHALL name a `.yml` template file or a directory containing exactly one. The template's prompts SHALL be taken from `prompts/<template stem>/*.md` beside it. The template SHALL be installed to `~/.flows/templates/<name>.yml` and its prompts to `~/.flows/prompts/<name>/`, where `<name>` is the stem or the `--as` name.

#### Scenario: Install by tag

- **WHEN** a user installs `<url>//ship@v1`
- **THEN** the lockfile records the commit `v1` pointed to, and `ship` appears in `flows templates list`.

#### Scenario: Ambiguous path

- **WHEN** the path is a directory with several `.yml` files
- **THEN** install fails and lists them.

### Requirement: Review before install

Before writing any file, install and update SHALL print a review of the template: source and commit, each phase with its type, every script command, bash allowlists and the `unrestricted`, `expose_credentials` and `offline` settings, `include:` entries, and prompt files. They SHALL proceed only after the user confirms, or when `--yes` is given. Update SHALL also show a diff of changed files.

#### Scenario: Declined review

- **WHEN** the user answers no at the review prompt
- **THEN** nothing is written and the lockfile is unchanged.

### Requirement: Names don't shadow silently

Install SHALL refuse a name that matches a builtin template or an existing global template not owned by the registry, unless `--as` gives another name. Reinstalling a registry-owned name SHALL require `update`.

#### Scenario: Builtin name

- **WHEN** a user installs a template whose stem is `feature`
- **THEN** install fails and suggests `--as`.

### Requirement: Manage installed templates

The system SHALL list installed templates with source, commit and trust; update one to the latest commit of its ref; remove one (its template file, prompts and lock entry); and set or revoke trust.

#### Scenario: Remove

- **WHEN** a user removes an installed template
- **THEN** its template file, prompts directory and lock entry are deleted.

### Requirement: Synced builtin copies are tracked

`flows templates sync` SHALL record a hash of each builtin template and prompt it writes under `~/.flows/`. On a later sync it SHALL update a copy whose content still matches its recorded hash, and SHALL keep a copy that was edited or has no record unless `--force` is given, listing what it kept. `flows doctor` SHALL list copies that differ from their builtin, separating outdated ones from those with no record and customized ones.

#### Scenario: Builtin changed, copy untouched

- **WHEN** a builtin prompt changes after sync wrote it and the copy was never edited
- **THEN** the next `flows templates sync` replaces the copy with the new builtin.

#### Scenario: Copy edited

- **WHEN** the user edited a synced copy
- **THEN** sync keeps it and names it, and `--force` replaces it.

#### Scenario: Copy from before records

- **WHEN** a copy differs from its builtin and has no record
- **THEN** sync keeps it and doctor warns that it may be stale.

