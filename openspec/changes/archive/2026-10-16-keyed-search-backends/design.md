## Decisions

- **Endpoints:** Brave `GET https://api.search.brave.com/res/v1/web/search?q&count` (`count` capped at the API's 20) with `X-Subscription-Token`; Serper `POST https://google.serper.dev/search` with JSON `{q, num}` and `X-API-KEY`. The body is serialized by hand, so reqwest's `json` feature isn't needed.
- **Key resolution:** config first, then env. Env lets users keep the key out of `config.yml`, and the sandbox clears env for shell commands either way. `backend_key_with` takes an injected env lookup, so tests don't mutate process env.
- **`base_url` reuse:** existing field, now also an endpoint override for keyed backends (testing, proxies). It stays out of the SSRF guard like SearXNG's: it's user configuration, not model-supplied.
- **No doctor probe:** each request costs quota, so doctor checks key presence only.
- **Tests:** pure parsers, plus a one-shot `std::net` mock server that checks method, query/body, and auth header end to end.
