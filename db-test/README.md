# db-test (optional)

- Docker compose for postgres/pgadmin is optional; embedded DB runtime can be used instead (`db.runtime.mode = "embedded"`).
- To run dockerized postgres:
  - `cp db-test/.env.example db-test/.env` and set the passwords (`db-test/.env` is git-ignored)
  - `cd db-test && docker-compose up -d`
- To prep embedded sqlite for CI/local:
  - `./scripts/db-runtime-init.sh`

