# KCIDB-ng

KCIDB-ng is fresh rewrite of our wrappers around the KernelCI Database (KCIDB). It provides services for submitting kernel test data to the KCIDB database via HTTP requests, and also
come with log analysis capabilities.

## Architecture

The system consists of several interconnected components:

1. **kcidb-restd-rs** - A Rust-based REST service that:
   - Receives JSON submissions via HTTP/HTTPS
   - Authenticates users via JWT
   - Stores valid submissions in a spool directory
   - Provides status endpoints for submissions

2. **ingester** - A Python service that:
   - Processes JSON submissions from the spool directory
   - Validates them against the KCIDB schema
   - Loads them into the KCIDB database
   - Archives processed submissions
   This service has been replaced with [KernelCI Dashboard's implementation of the ingester](https://github.com/kernelci/dashboard/blob/main/backend/kernelCI_app/management/commands/monitor_submissions.py) and was left here just as a temporary and standalone usage.

3. **logspec-worker** - A Python service that:
   - Monitors the database for failed tests and builds
   - Downloads and analyzes log files using the logspec library
   - Identifies issues and incidents from logs
   - Submits findings back to KCIDB

4. **PostgreSQL Database** - Stores all KCIDB data
   - Can be run locally (self-hosted mode)
   - Can be connected to Google Cloud SQL (deprecated soon)

## Installation

### Prerequisites

- Docker and Docker Compose (the newer `docker compose` plugin, not the legacy `docker-compose`; version 2.0+ required)
- Git

### Clone the Repository

```bash
git clone https://github.com/kernelci/kcidb-ng.git
cd kcidb-ng
```

### Quick Start (recommended)
To quickly start the KCIDB-ng services with a local PostgreSQL database, run:

```bash
./self-hosted.sh run
```
This script will:
- Build and start the Docker containers
- Initialize the PostgreSQL database
- Start the REST API, ingester, and logspec-worker services

Also available commands:
- `./self-hosted.sh down` - Stops the services
- `./self-hosted.sh clean` - Stops and removes all containers, configs, databases, networks, and volumes

### Manual configuration

Create a `.env` file in the root directory with the following environment variables:

```
# PostgreSQL configuration
POSTGRES_PASSWORD=kcidb
PS_PASS=kcidb
PG_URI=postgresql:dbname=kcidb user=kcidb_editor password=kcidb host=db port=5432
# JWT authentication
JWT_SECRET=your_jwt_secret
```

## Usage

### Starting the Services

#### Self-hosted Mode (with local PostgreSQL)

The self-hosted profile includes a local PostgreSQL database and an initialization service:

```bash
sudo docker compose --profile=self-hosted up -d --build
```

This command:
- Builds and starts all necessary containers
- Sets up a local PostgreSQL database
- Initializes the database schema
- Starts the REST API, ingester, and logspec-worker services

Note: By default it is expecting PostgreSQL to be running with default settings, except postgres password which is set to `kcidb`.
It will also create a user `kcidb_editor` with password `kcidb` and a database `kcidb`, and user `kcidb_viewer` with password `kcidb` for read-only access.

#### Google Cloud SQL Mode

If you prefer to use Google Cloud SQL as your database:

```bash
docker compose --profile=google-cloud-sql up -d --build
```

Make sure to provide the appropriate credentials in your `.env` file.

### Generating tokens

If your kcidb-ng is installed in isolated environment, you can disable JWT authentication by commenting out the JWT command in `docker-compose.yaml`:

```yaml
#    command: ["/usr/local/bin/kcidb-restd-rs","-j",""]
```

If you want to use JWT authentication, you can generate a token using the following command:

```bash
kcidb-restd-rs/tools/jwt_rest.py --secret YOUR_SECRET --origin YOUR_ORIGIN
```

### Validating tokens

To validate a JWT token, you can use the following command:

```bash
curl -X GET \
  -H "Authorization: Bearer <jwt_token>" \
  https://localhost:443/authtest
```
This will return a JSON response with the token's validity.

### Sending Data to the API

To submit data to the REST API:

```bash
curl -X POST \
  -H "Authorization: Bearer <jwt_token>" \
  -H "Content-Type: application/json" \
  -d @submission.json \
  https://localhost:443/submit
```

### Checking Status

You can check the status of your submission using:

```bash
curl -X GET   -H "Authorization: Bearer <yourtoken>" https://staging.db.kernelci.org/status?id=wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz
{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"failed","message":"File found"}
```

Possible status values:

-	{"id":"0","status":"error","message":"Empty id"}	The request was invalid (e.g., missing or invalid ID, authentication error, etc.).
-	{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"inprogress","message":"File still in progress"}	A submission file with .json.temp exists, indicating the upload is not yet complete.
-	{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"ready","message":"File waiting for processing"}	The submission file exists and is ready for processing, but not yet archived or failed.
-	{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"processed","message":"File archived"}	The submission has been processed and archived.
-	{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"failed","message":"File failed to pass validation"}	The submission failed validation and is in the failed directory.
-	{"id":"wvTu6myNQOlM7IEWhHJz8WnnE0GTG1yz","status":"notfound","message":"File not found"}	No submission file with the given ID was found in any expected location.


## Directory Structure

- `/spool`: Stores incoming submissions (managed by docker volumes)
  - `/spool/failed`: Stores submissions that failed to process
  - `/spool/archive`: Stores successfully processed submissions

- `/state`: Stores application state (managed by docker volumes)
  - `processed_builds.db`: Tracks processed builds
  - `processed_tests.db`: Tracks processed tests

- `/cache`: Caches downloaded log files for logspec-worker

## Development and Debugging

### Viewing Logs

```bash
docker logs kcidb-rest
docker logs ingester
docker logs logspec-worker
docker logs postgres
```

### Connecting to the Database

```bash
docker exec -it postgres psql -U kcidb_editor -d kcidb
```

### Authentication

The REST API uses JWT for authentication. To disable JWT authentication (not recommended for production):

Uncomment this line in docker-compose.yaml:
```yaml
#    command: ["/usr/local/bin/kcidb-restd-rs","-j",""]
```

### Manual Log Processing

To manually process a log file through logspec without submitting it to the database, you can run:

```bash
docker exec -it logspec-worker python /app/logspec_worker.py --spool-dir /app/spool --origins microsoft --dry-run
```

## License

This project is licensed under the [LGPL-2.1 license](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.en.html).

