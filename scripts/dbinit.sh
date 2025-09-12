#!/bin/bash

# Wait for the database to be ready
echo "Waiting for the database to be ready..."
while ! pg_isready -h db -U postgres; do
  sleep 1
done
echo "Database is ready."
# set env variable for default password set in docker-compose
export PGPASSWORD="kcidb"
# verify if the database is already initialized
if psql -h db -U postgres -tAc "SELECT 1 FROM pg_database WHERE datname='kcidb'" | grep -qw 1; then
  echo "Database kcidb already exists. Skipping initialization."
  exit 0
fi

echo "Initializing database..."
psql -h db -U postgres -c "CREATE ROLE kcidb WITH LOGIN PASSWORD 'kcidb';"
echo "Creating database user kcidb..."
psql -h db -U postgres -c "CREATE DATABASE kcidb WITH OWNER kcidb;"
echo "Database kcidb created."

echo "Creating database roles..."
psql -h db -U postgres -c "CREATE ROLE kcidb_editor WITH LOGIN PASSWORD 'kcidb';"
psql -h db -U postgres -d kcidb -c \
  "ALTER SCHEMA public OWNER TO kcidb;
   GRANT USAGE, CREATE ON SCHEMA public TO kcidb, kcidb_editor;"
psql -h db -U postgres -c "GRANT ALL PRIVILEGES ON DATABASE kcidb TO kcidb_editor;"
psql -h db -U postgres -c "GRANT USAGE, CREATE ON SCHEMA public TO kcidb_editor;"
psql -h db -U postgres -c "GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO kcidb_editor;"
psql -h db -U postgres -c "GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO kcidb_editor;"
psql -h db -U postgres -c "GRANT ALL PRIVILEGES ON ALL FUNCTIONS IN SCHEMA public TO kcidb_editor;"

echo "Creating DB schema for kcidb..."
echo "PG_URI: ${PG_URI}"
kcidb-db-init -d "${PG_URI}" --ignore-initialized

# add kcidb_viewer who can read only
psql -h db -U postgres -c "CREATE ROLE kcidb_viewer WITH LOGIN PASSWORD 'kcidb';"
psql -h db -U postgres -c "GRANT CONNECT ON DATABASE kcidb TO kcidb_viewer;"
psql -h db -U postgres -c "GRANT USAGE ON SCHEMA public TO kcidb_viewer;"
psql -h db -U postgres -c "GRANT SELECT ON ALL TABLES IN SCHEMA public TO kcidb_viewer;"
psql -h db -U postgres -c "GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO kcidb_viewer;"
echo "Database initialized."
echo "DB schema created."
