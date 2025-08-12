#!/bin/bash

# if first argument is "down" then run docker compose down
if [ "$1" == "down" ]; then
  docker compose --profile=self-hosted down
  echo "Docker containers stopped and removed."
  exit 0
fi

# Clean command will remove all containers, volumes, and networks
# So you can install everything from scratch
if [ "$1" == "clean" ]; then
  # Ask user for confirmation
  read -p "Are you sure you want to remove all kcidb-ng Docker containers, volumes, and networks? This will delete all data. (y/N): " confirm
  if [[ ! $confirm =~ ^[yY]$ ]]; then
    echo "Operation cancelled."
    exit 0
  fi

  docker compose --profile=self-hosted down --volumes --remove-orphans
  rm -rf ./config/* ./logspec-worker/logspec_worker.yaml ./db .env
  echo "Docker containers, volumes, and networks removed."
  exit 0
fi

if [ "$1" == "run" ]; then
  # If .env file does not exist, create it with default values
  if [ ! -f .env ]; then
    echo ".env file not found, creating a new one..."
    RND_JWT_SECRET=$(openssl rand -hex 32)
    echo "# PostgreSQL configuration
POSTGRES_PASSWORD=kcidb
PS_PASS=kcidb
PG_URI=postgresql:dbname=kcidb user=kcidb_editor password=kcidb host=db port=5432
# Programs will be more talkative if this is set, in production might want to set to 0
KCIDB_VERBOSE=1
# logspec will not modify anything in database if this is set
KCIDB_DRY_RUN=1
# JWT authentication
JWT_SECRET=${RND_JWT_SECRET}" > .env
    echo "New .env file created with default values."
  else
    echo ".env file already exists, skipping creation."
  fi

  docker compose --profile=self-hosted up -d --build

  if [ ! -f config/logspec_worker.yaml ]; then
      echo "logspec_worker.yaml not found, copying example"
      cp logspec-worker/logspec_worker.yaml.example config/logspec_worker.yaml
  fi
  echo "Docker containers started and running in detached mode."
  exit 0
fi

# If no arguments are provided, show usage
echo "Usage: $0 [down|clean|run]"
echo "  down   - Stop and remove Docker containers"
echo "  clean  - Stop and remove all Docker containers, volumes, and networks"
echo "  run    - Start Docker containers in detached mode"
