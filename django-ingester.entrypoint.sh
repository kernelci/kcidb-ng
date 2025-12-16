#!/bin/sh

export DB_DEFAULT="{
    \"ENGINE\": \"${DB_DEFAULT_ENGINE:=django.db.backends.postgresql}\",
    \"NAME\": \"${DB_DEFAULT_NAME:=kcidb}\",
    \"USER\": \"${DB_DEFAULT_USER:=kcidb_editor}\",
    \"PASSWORD\": \"$DB_DEFAULT_PASSWORD\",
    \"HOST\": \"${DB_DEFAULT_HOST:=db}\",
    \"PORT\": \"${DB_DEFAULT_PORT:=5432}\",
    \"CONN_MAX_AGE\": ${DB_DEFAULT_CONN_MAX_AGE:=null},
    \"OPTIONS\": {
      \"connect_timeout\": ${DB_DEFAULT_TIMEOUT:=16}
    }
}"

AGGREGATION_ENABLED=$(echo "$AGGREGATION_ENABLED" | tr '[:upper:]' '[:lower:]')
AGGREGATION_INTERVAL=${AGGREGATION_INTERVAL:=60}
if [ "$AGGREGATION_ENABLED" = "true" ]
then
    python manage.py process_pending_aggregations --batch-size=10000 --loop --interval=$AGGREGATION_INTERVAL &
fi

exec "$@"
