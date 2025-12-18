# db-test (optional)

- Docker compose for postgres/pgadmin is optional; embedded DB runtime can be used instead (`db.runtime.mode = "embedded"`).
- To run dockerized postgres:
  - `cd db-test && docker-compose up -d`
- To prep embedded sqlite for CI/local:
  - `./scripts/db-runtime-init.sh`

