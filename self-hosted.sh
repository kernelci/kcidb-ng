#!/bin/bash
# if first argument is "down" then run docker compose down
if [ "$1" == "down" ]; then
  docker compose -f docker-compose-all.yaml --profile=self-hosted down
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

  docker compose -f docker-compose-all.yaml --profile=self-hosted down --volumes --remove-orphans
  rm -rf ./config/* ./logspec-worker/logspec_worker.yaml ./db .env
  echo "Docker containers, volumes, and networks removed."
  exit 0
fi

if [ "$1" == "update" ]; then
  cd dashboard
  git pull --ff
  cd ..
  docker compose -f docker-compose-all.yaml --profile=self-hosted pull
  docker compose -f docker-compose-all.yaml --profile=self-hosted build dashboard
  docker compose -f docker-compose-all.yaml --profile=self-hosted up -d --build
  exit 0
fi

if [[ "$1" == "run" || "$1" == "up" ]]; then
  # Check if dashboard cloned
  if [ ! -d dashboard ]; then
    git clone https://github.com/kernelci/dashboard
    if [ $? -ne 0 ]; then
      echo "Failed to clone dashboard repository."
      exit 1
    fi
  fi
  # If .env file does not exist, create it with default values
  if [ ! -f .env ]; then
    echo ".env file not found, creating a new one..."
    RND_JWT_SECRET=$(openssl rand -hex 32)
    PG_PASS="kcidb"
    echo "# PostgreSQL configuration
POSTGRES_PASSWORD=kcidb
PG_PASS=kcidb
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

  if [ ! -f dashboard/.env.backend ]; then
    echo "Creating dashboard/.env.backend file..."
    cp scripts/dashboard.env.backend dashboard/.env.backend
  fi

  if [ ! -f dashboard/.env.db ]; then
    echo "Creating dashboard/.env.db file..."
    cp dashboard/.env.db.example dashboard/.env.db
  fi

  if [ ! -f dashboard/.env.proxy ]; then
    echo "Creating dashboard/.env.proxy file..."
    cp dashboard/.env.proxy.example dashboard/.env.proxy
  fi

  if [ ! -d dashboard/backend/runtime/secrets/ ]; then
    echo "Creating secrets directory and file..."
    mkdir -p dashboard/backend/runtime/secrets/
    echo "$PG_PASS" >dashboard/backend/runtime/secrets/postgres_password_secret
  fi

  docker compose -f docker-compose-all.yaml --profile=self-hosted up -d --build

  if [ ! -f config/logspec_worker.yaml ]; then
      echo "logspec_worker.yaml not found, copying example"
      cp logspec-worker/logspec_worker.yaml.example config/logspec_worker.yaml
  fi
  echo "Docker containers started and running in detached mode."
  exit 0
fi

if [ "$1" == "logs" ]; then
  docker compose -f docker-compose-all.yaml --profile=self-hosted logs -f
  exit 0
fi

# If no arguments are provided, show usage
echo "This script will install fully operational self-hosted instance of kcidb-ng and KernelCI dashboard"
echo "Usage: $0 [down|clean|run|up|logs|update"
echo "  down      - Stop and remove Docker containers"
echo "  clean     - Stop and remove all Docker containers, volumes, and networks"
echo "  run or up - Start Docker containers in detached mode"
echo "  logs      - View logs for all Docker containers"
echo "  update    - Update components to latest versions"