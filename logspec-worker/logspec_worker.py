#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-only
# Copyright (C) 2025 Collabora Ltd
# Author: Denys Fedoryshchenko <denys.f@collabora.com>
#
# This library is free software; you can redistribute it and/or modify it under
# the terms of the GNU Lesser General Public License as published by the Free
# Software Foundation; version 2.1.
#
# This library is distributed in the hope that it will be useful, but WITHOUT ANY
# WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
# PARTICULAR PURPOSE. See the GNU Lesser General Public License for more details.
#
# You should have received a copy of the GNU Lesser General Public License along
# with this library; if not, write to the Free Software Foundation, Inc.,
# 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA

"""
Search kcidb tests for unprocessed tests and process them over logspec, update entries
"""

import os
import sys
import json

# postgres
import psycopg2
from psycopg2 import sql
from psycopg2.extras import DictCursor
import datetime
import shelve
import gzip

# md5
import hashlib
import requests
from logspec_api import generate_issues_and_incidents
import kcidb
import argparse
import time
import yaml
import fnmatch
import time
import logging

APP_STATE_DIR = "/app/state"
TESTS_STATE_DB = os.path.join(APP_STATE_DIR, "processed_tests.db")
BUILDS_STATE_DB = os.path.join(APP_STATE_DIR, "processed_builds.db")
logging.basicConfig(level=logging.INFO)
PROCESS_DAYS = 1

class LogspecState:
    """
    Class to manage the state of the logspec
    """

    def __init__(self):
        self.config_file = None
        self._cfg = None
        self._spool_dir = None
        self.dry_run = False

    def load_config(self, config_file):
        """
        Load the logspec configuration
        """
        self.config_file = config_file
        if not os.path.exists(self.config_file):
            logging.warning(f"Config file {self.config_file} does not exist")
            # on early stage we make it backward compatible with
            # instances without config file
            logging.warning("No config file found, using default values")
            self._cfg = {}
            return
        
        try:
            with open(self.config_file, "r") as f:
                self._cfg = yaml.safe_load(f)
        except Exception as e:
            logging.error(f"Error loading config file {self.config_file}: {e}")
            sys.exit(1)

    def validate_path(self, path, include_path):
        """
        Validate the path against the include_path
        """
        # compare path with include_path (include_path might have wildcards like *)
        # fnmatch is closest to shell wildcard matching
        #logging.info(f"Validating path {path} against {include_path}")
        # if include_path is string, convert it to list
        if isinstance(include_path, str):
            include_path = [include_path]
        # if include_path is list, iterate over it
        if isinstance(include_path, list):
            for path_element in include_path:
                if fnmatch.fnmatch(path, path_element):
                    return True
            return False
        else:
            logging.error(f"include_path {include_path} is not a string or list")
            sys.exit(1)


    def is_processable(self, node, type):
        """
        Check if the node is processable
        """
        node_origin = node["origin"]
        if node_origin not in self._cfg:
            logging.warning(f"Origin {node_origin} not found in config file: {self._cfg}")
            return False
        # iterate over origin in config file
        for entry in self._cfg[node_origin]:
            if entry["type"] == type:
                if 'include_path' in entry:
                    if self.validate_path(node["path"], entry["include_path"]):
                        return True
                    return False
                return True
        return False

    def get_origins(self):
        """
        Get the origins from the config file
        """
        if self._cfg is None:
            logging.warning("Config file not loaded")
            return []
        # first level keys is the origins
        origins = []
        for key in self._cfg.keys():
            origins.append(key)
        return origins

    def set_spool_dir(self, spool_dir):
        """
        Set the spool directory
        """
        self._spool_dir = spool_dir
        if not os.path.exists(self._spool_dir):
            os.makedirs(self._spool_dir)
            logging.info(f"Spool directory {self._spool_dir} created")
        return self._spool_dir

    def get_spool_dir(self):
        """
        Get the spool directory
        """
        if self._spool_dir is None:
            logging.warning("Spool directory not set")
            return None
        return self._spool_dir




def set_test_processed(cursor, test_id):
    """
    Mark the test as processed in the shelve database
    """
    # shelve database
    with shelve.open(TESTS_STATE_DB) as db:
        if test_id not in db:
            db[test_id] = True
            logging.info(f"Test {test_id} marked as processed")
        else:
            logging.info(f"Test {test_id} already processed")


def is_test_processed(test_id):
    """
    Check if the test is already processed
    """
    with shelve.open(TESTS_STATE_DB) as db:
        if test_id in db:
            return True
        else:
            return False


def set_build_processed(cursor, build_id):
    """
    Mark the build as processed in the shelve database
    """
    # shelve database
    with shelve.open(BUILDS_STATE_DB) as db:
        if build_id not in db:
            db[build_id] = True
            logging.info(f"Build {build_id} marked as processed")
        else:
            logging.info(f"Build {build_id} already processed")


def is_build_processed(build_id):
    """
    Check if the build is already processed
    """
    with shelve.open(BUILDS_STATE_DB) as db:
        if build_id in db:
            return True
        else:
            return False


def get_db_connection():
    """
    Connect to the PostgreSQL database
    """
    # pg_dsn = "postgresql://kcidb:kcidb@localhost:5432/kcidb"
    # check .pg_dsn file
    pg_dsn = os.environ.get("PG_URI")

    if os.path.exists(".pg_dsn"):
        with open(".pg_dsn", "r") as f:
            pg_dsn = f.read().strip()

    # Check if the environment variable is set
    if pg_dsn is None:
        pg_dsn = os.environ.get("PG_DSN")
        if pg_dsn is None:
            logging.error(
                "No database connection string found. Please set the PG_DSN environment variable or create a .pg_dsn file."
            )
            sys.exit(1)

    # if pg_dsn contains prefix postgresql: (kcidb specific) - remove it
    if pg_dsn.startswith("postgresql:"):
        pg_dsn = pg_dsn[len("postgresql:") :]

    try:
        conn = psycopg2.connect(dsn=pg_dsn, cursor_factory=DictCursor)
        logging.info("Connected to database")
        return conn
    except Exception as e:
        logging.error(f"Error connecting to database: {e}")
        sys.exit(1)


def get_unprocessed_tests(cursor, origins):
    """
    Get unprocessed tests from the database
    """
    duration = datetime.datetime.now() - datetime.timedelta(days=PROCESS_DAYS)
    #
    try:
        query = (
            "SELECT * FROM tests WHERE _timestamp > %s AND log_url IS NOT NULL"
            " AND status != 'PASS' AND origin = ANY(%s)"
        )
        cursor.execute(query, (duration, origins))
        tests = cursor.fetchall()
        #logging.info(f"SQL {query} get_unprocessed_tests: {tests} origins: {origins}")
        return tests
    except Exception as e:
        logging.error(f"Error fetching unprocessed tests: {e}")
        raise e


def get_unprocessed_builds(cursor, origins):
    """
    Get unprocessed builds from the database
    """
    duration = datetime.datetime.now() - datetime.timedelta(days=PROCESS_DAYS)
    #
    try:
        query = (
            "SELECT * FROM builds WHERE _timestamp > %s AND log_url IS NOT NULL"
            " AND status != 'PASS' AND origin = ANY(%s)"
        )
        cursor.execute(query, (duration, origins))
        builds = cursor.fetchall()
        #logging.info(f"SQL {query} get_unprocessed_builds: {builds} origins: {origins}")
        return builds
    except Exception as e:
        logging.error(f"Error fetching unprocessed builds: {e}")
        return []


def fetch_log_id(log_url):
    # generate ID as hash of URL
    log_id = hashlib.md5(log_url.encode()).hexdigest()
    # check if log_id exists in cache
    cache_file = os.path.join("/cache", log_id)
    if os.path.exists(cache_file):
        return log_id
    # fetch log from URL
    try:
        response = requests.get(log_url)
        if response.status_code == 200:
            # save log to cache
            with open(cache_file, "wb") as f:
                f.write(response.content)
            logging.info(f"Log {log_id} fetched and saved to cache")
            # decompress log if gzipped
            if log_url.endswith(".gz"):
                # rename to .gz
                os.rename(cache_file, cache_file + ".gz")
                with gzip.open(cache_file + ".gz", "rb") as f_in:
                    with open(cache_file, "wb") as f_out:
                        f_out.write(f_in.read())
                os.remove(cache_file + ".gz")
            return log_id
        else:
            logging.error(f"Error fetching log {log_url}: {response.status_code}")
            return None
    except Exception as e:
        logging.error(f"Error fetching log {log_url}: {e}")
        return None


def logspec_process_node(cursor, node, kind):
    """
    Process the test over logspec
    Allowed values for kind are: build, boot, test
    """
    log_url = node["log_url"]
    log_id = fetch_log_id(log_url)
    logging.info(f"Log ID: {log_id}")
    # check if log_id is None
    if log_id is None:
        logging.error(f"Error fetching log {log_url}")
        return
    log_file = os.path.join("/cache", log_id)
    return generate_issues_and_incidents(cursor, node["id"], log_file, kind, node["origin"])


def remove_none_fields(data):
    """Remove all keys with `None` values as KCIDB doesn't allow it"""
    if isinstance(data, dict):
        return {
            key: remove_none_fields(val) for key, val in data.items() if val is not None
        }
    if isinstance(data, list):
        return [remove_none_fields(item) for item in data]
    return data


def submit_to_kcidb(issues, incidents, spool_dir):
    """
    Submit issues and incidents to kcidb
    """
    revision = {
        "checkouts": [],
        "builds": [],
        "tests": [],
        "issues": issues,
        "incidents": incidents,
        "version": {"major": 4, "minor": 5},
    }
    # remove None fields
    revision = remove_none_fields(revision)
    # generate raw json
    raw_json = json.dumps(revision)
    # random part of filename
    random_part = os.urandom(8).hex()
    # generate filename
    filename = f"logspec_{random_part}.json.temp"
    # drop it as json in spool_dir
    with open(os.path.join(spool_dir, filename), "w") as f:
        f.write(raw_json)
    # rename it to .json
    os.rename(
        os.path.join(spool_dir, filename),
        os.path.join(spool_dir, filename[:-5] + ".json"),
    )


def process_tests(cursor, state):
    """
    Process the tests
    """
    spool_dir = state.get_spool_dir()
    origins = state.get_origins()
    # code to get unprocessed tests
    unprocessed_tests = get_unprocessed_tests(cursor, origins)
    if not unprocessed_tests:
        logging.info("No unprocessed tests found")
        return
    # print the unprocessed tests
    for test in unprocessed_tests:
        if not state.is_processable(test, "test"):
            logging.info(f"Test {test['id']} with path {test['path']} is not processable")
            if not state.dry_run:
                set_test_processed(cursor, test["id"])
            continue

        # print formatted column names and values
        for column, value in test.items():
            logging.info(f"{column}: {value}")
        # Check if the test is already processed
        if is_test_processed(test["id"]):
            logging.info(f"Test {test['id']} already processed")
            continue
        # Process the test
        logging.info(f"Processing test {test['id']}")
        # Call logspec, we assume all tests are of type "boot"
        # TODO: We need different types of parsers for different tests
        res_nodes, new_status = logspec_process_node(cursor, test, "boot")
        if res_nodes["issue_node"] or res_nodes["incident_node"]:
            if not state.dry_run:
                # submit to kcidb incident and issue
                logging.info(f"Submitting to kcidb")
                submit_to_kcidb(
                    res_nodes["issue_node"], res_nodes["incident_node"], spool_dir
                )
            else:
                logging.info("Dry run - not submitting tests to kcidb, just printing")
                logging.info(json.dumps(res_nodes, indent=4))

        # mark the test as processed (TODO: must be in database)
        if not state.dry_run:
            set_test_processed(cursor, test["id"])


def process_builds(cursor, state):
    """
    Process the builds
    """
    spool_dir = state.get_spool_dir()
    origins = state.get_origins()
    # get unprocessed builds
    unprocessed_builds = get_unprocessed_builds(cursor, origins)
    if not unprocessed_builds:
        logging.info("No unprocessed builds found")
        return
    for build in unprocessed_builds:
        if not state.is_processable(build, "build"):
            logging.info(f"Build {build['id']} is not processable")
            if not state.dry_run:
                set_build_processed(cursor, build["id"])
            continue
        # print formatted column names and values
        for column, value in build.items():
            logging.info(f"{column}: {value}")
        # Check if the build is already processed
        if is_build_processed(build["id"]):
            logging.info(f"Build {build['id']} already processed")
            continue
        # Process the build
        logging.info(f"Processing build {build['id']}")
        # Call logspec
        res_nodes, new_status = logspec_process_node(cursor, build, "build")
        if res_nodes["issue_node"] or res_nodes["incident_node"]:
            # submit to kcidb incident and issue
            if not state.dry_run:
                logging.info(f"Submitting to kcidb")
                submit_to_kcidb(
                    res_nodes["issue_node"], res_nodes["incident_node"], spool_dir
                )
            else:
                logging.info("Dry run - not submitting builds to kcidb, just printing")
                logging.info(json.dumps(res_nodes, indent=4))
        # mark the build as processed (TODO: must be in database)
        if not state.dry_run:
            set_build_processed(cursor, build["id"])


def verify_appstate():
    """
    Verify if the appstate directories exist
    """
    if not os.path.exists("/app/state"):
        os.makedirs("/app/state")
    if not os.path.exists(TESTS_STATE_DB):
        with shelve.open(TESTS_STATE_DB) as db:
            db["processed_tests"] = {}
    if not os.path.exists(BUILDS_STATE_DB):
        with shelve.open(BUILDS_STATE_DB) as db:
            db["processed_builds"] = {}


def main():
    """
    Main function to process the logspec
    """
    # sleep 1 second
    logging.info("Starting logspec worker")
    state = LogspecState()
    parser = argparse.ArgumentParser()
    parser.add_argument("--spool-dir", type=str, required=True)
    parser.add_argument("--dry-run", action="store_true", help="dry run")
    parser.add_argument(
        "--config-file",
        type=str,
        default="config/logspec_worker.yaml",
        help="logspec config file",
    )
    args = parser.parse_args()
    state.load_config(args.config_file)
    state.set_spool_dir(args.spool_dir)
    if args.dry_run:
        logging.warning("Running in dry run mode, not submitting to kcidb")
        logging.warning("WARNING: Dry run will not set internal state as processed")
        logging.warning("To avoid expensive reprocessing, it will process only once")
        state.dry_run = True
    # verify if cache directory exists
    if not os.path.exists("/cache"):
        os.makedirs("/cache")
    # verify if appstate directory and db exists
    verify_appstate()

    # Connect to the database
    conn = get_db_connection()
    cursor = conn.cursor()

    logging.info("Connected to database, starting processing")
    while True:
        process_builds(cursor, state)
        process_tests(cursor, state)
        # sleep 60 seconds
        if args.dry_run:
            logging.warning("Dry run - sleeping 6 hours")
            time.sleep(6 * 60 * 60)
        time.sleep(60)

    conn.close()
    cursor.close()


if __name__ == "__main__":
    main()
