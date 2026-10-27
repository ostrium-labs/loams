# Changelog (deploy/byoc/aws)

- First version (BY0 Task 3): cloud-init, secrets fetch, instance policy. Supports Loams 0.1.1 (the light default stack).
- CC1 Task 6 (design 80, D1827): `CONFLUENT_INTEGRATIONS` in `config.env` (`--confluent-integrations`); the secrets fetcher writes each integration's Confluent keys to `secrets/connect/<ns>/confluent-<i>/` (mode 600), never to `.env`.
