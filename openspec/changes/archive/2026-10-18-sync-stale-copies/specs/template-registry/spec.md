## ADDED Requirements

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
