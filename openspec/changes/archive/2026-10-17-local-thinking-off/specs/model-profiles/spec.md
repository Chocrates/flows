## ADDED Requirements

### Requirement: Turning reasoning off on local servers

When a phase's resolved `thinking` setting is `false` and its model is served by an OpenAI-compatible endpoint other than `api.openai.com`, each request SHALL carry `chat_template_kwargs` with `enable_thinking: false`. Requests SHALL NOT carry it when thinking is enabled or unset, or when the endpoint is `api.openai.com`.

#### Scenario: Phase opts out on a local server

- **WHEN** a phase sets `thinking: false` and its profile points at a local OpenAI-compatible server
- **THEN** its requests include `chat_template_kwargs: {enable_thinking: false}`.

#### Scenario: Default unchanged

- **WHEN** no layer sets `thinking`
- **THEN** no `chat_template_kwargs` is sent.

#### Scenario: Official OpenAI API

- **WHEN** a phase sets `thinking: false` and its endpoint is `api.openai.com`
- **THEN** no `chat_template_kwargs` is sent.
