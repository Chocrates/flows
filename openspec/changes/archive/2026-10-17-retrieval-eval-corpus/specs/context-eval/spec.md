## MODIFIED Requirements

### Requirement: Deterministic retrieval evaluation

The system SHALL provide a retrieval evaluation that, given labeled fixtures
mapping a task to the repository files/symbols that are actually relevant,
measures whether the enabled retrieval features (search, repo map, semantic
retrieval) surface those targets — as recall@k / MRR — without invoking the
language model, so retrieval quality is measured deterministically.

The evaluation SHALL also accept retrieval corpora: one repository with many
labeled queries, scored per corpus and per query on request, with the overall
mean weighting every query equally. A corpus label naming a file that is not in
its repository SHALL be rejected.

#### Scenario: Recall measured without a model call

- **WHEN** the retrieval eval runs over a labeled fixture with a feature enabled
- **THEN** it reports recall@k (and/or MRR) for that feature's ranking against the labeled targets, making no language-model request.

#### Scenario: Corpus scored per query

- **WHEN** the retrieval eval runs over a corpus of several queries
- **THEN** it reports the corpus mean and query count for each ranker, and a row per query when asked.

#### Scenario: Bad label

- **WHEN** a corpus query lists a relevant file that does not exist in the corpus repository
- **THEN** the eval stops with an error naming the file.
