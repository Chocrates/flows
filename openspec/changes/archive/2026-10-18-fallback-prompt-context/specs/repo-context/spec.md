## ADDED Requirements

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
