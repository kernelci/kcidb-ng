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

import kcidb
import tempfile
import os
import argparse
from kcidb import io, db, mq, orm, oo, monitor, tests, unittest, misc # noqa
import json
import time
import logging
import yaml

# default database
DATABASE = "postgresql:dbname=kcidb user=kcidb password=kcidb host=localhost port=5432"
VERBOSE = 0

logger = logging.getLogger('ingester')

def get_db_credentials():
    global DATABASE
    # if PG_URI present - use it instead of default DATABASE
    pg_uri = os.environ.get("PG_URI")
    if pg_uri:
        DATABASE = pg_uri
    pgpass = os.environ.get("POSTGRES_PASSWORD")
    if not pgpass:
        raise Exception("POSTGRES_PASSWORD environment variable not set")
    (pgpass_fd, pgpass_filename) = tempfile.mkstemp(suffix=".pgpass")
    with os.fdopen(pgpass_fd, mode="w", encoding="utf-8") as pgpass_file:
        pgpass_file.write(pgpass)
    os.environ["PGPASSFILE"] = pgpass_filename
    db_uri = os.environ.get("PG_DSN")
    if db_uri:
        DATABASE = db_uri


def get_db_client(database):
    get_db_credentials()
    db = kcidb.db.Client(database)
    return db


def move_file_to_failed_dir(filename, failed_dir):
    try:
        os.rename(filename, os.path.join(failed_dir, os.path.basename(filename)))
    except Exception as e:
        print(f"Error moving file {filename} to failed directory: {e}")
        raise e

TREES_FILE = "/app/trees.yml"

def load_trees_name():
    with open(TREES_FILE, "r", encoding="utf-8") as f:
        data = yaml.safe_load(f)

    trees_name = {
        v["url"]: tree_name
        for tree_name, v in data.get("trees", {}).items()
    }

    return trees_name


def standardize_trees_name(input_data, trees_name):
    """ Standardize tree names in input data using the provided mapping """

    for checkout in input_data.get("checkouts", []):
        git_url = checkout.get("git_repository_url")
        if git_url in trees_name:
            correct_tree = trees_name[git_url]
            if checkout.get("tree_name") != correct_tree:
                checkout["tree_name"] = correct_tree

    return input_data


def ingest_submissions(spool_dir, trees_name, db_client=None):
    failed_dir = os.path.join(spool_dir, "failed")
    archive_dir = os.path.join(spool_dir, "archive")
    if db_client is None:
        raise Exception("db_client is None")
    io_schema = db_client.get_schema()[1]
    # iterate over all files in the directory spool_dir
    for filename in os.listdir(spool_dir):
        # skip directories
        if os.path.isdir(os.path.join(spool_dir, filename)):
            continue
        # skip if not json
        if not filename.endswith(".json"):
            continue
        logger.info(f"Ingesting {filename}")
        try:
            with open(os.path.join(spool_dir, filename), "r") as f:
                fsize = os.path.getsize(os.path.join(spool_dir, filename))
                if fsize == 0:
                    if VERBOSE:
                        logger.info(f"File {filename} is empty, skipping, deleting")
                    os.remove(os.path.join(spool_dir, filename))
                    continue
                start_time = time.time()
                if VERBOSE:
                    logger.info(f"File size: {fsize}")
                try:
                    data = json.loads(f.read())
                    data = standardize_trees_name(data, trees_name)
                    data = io_schema.validate(data)
                    data = io_schema.upgrade(data, copy=False)
                    db_client.load(data)
                except Exception as e:
                    logger.error(f"Error loading data: {e}")
                    logger.error(f"File: {filename}")
                    # move the file to the failed directory for later inspection
                    move_file_to_failed_dir(os.path.join(spool_dir, filename), failed_dir)
                    continue
                ing_speed = fsize / (time.time() - start_time) / 1024
                if VERBOSE:
                    logger.info(f"Ingested {filename} in {ing_speed} KB/s")
                # Archive the file
                os.rename(os.path.join(spool_dir, filename), os.path.join(archive_dir, filename))

        except Exception as e:
            logger.error(f"Error: {e}")
            logger.error(f"File: {filename}")
            raise e

def verify_dir(dir):
    if not os.path.exists(dir):
        logger.error(f"Directory {dir} does not exist")
        # try to create it
        try:
            os.makedirs(dir)
            logger.info(f"Directory {dir} created")
        except Exception as e:
            logger.error(f"Error creating directory {dir}: {e}")
            raise e
    if not os.path.isdir(dir):
        raise Exception(f"Directory {dir} is not a directory")
    if not os.access(dir, os.W_OK):
        raise Exception(f"Directory {dir} is not writable")
    logger.info(f"Directory {dir} is valid and writable")

def verify_spool_dirs(spool_dir):
    failed_dir = os.path.join(spool_dir, "failed")
    archive_dir = os.path.join(spool_dir, "archive")
    verify_dir(spool_dir)
    verify_dir(failed_dir)
    verify_dir(archive_dir)


def main():
    global VERBOSE
    # read from environment variable KCIDB_VERBOSE
    VERBOSE = int(os.environ.get("KCIDB_VERBOSE", 0))
    if VERBOSE:
        logging.basicConfig(level=logging.INFO)
    else:
        logging.basicConfig(level=logging.WARNING)
    parser = argparse.ArgumentParser()
    parser.add_argument("--spool-dir", type=str, required=True)
    parser.add_argument("--verbose", type=int, default=VERBOSE)
    args = parser.parse_args()
    logger.info("Starting ingestion process...")
    verify_spool_dirs(args.spool_dir)
    trees_name = load_trees_name()
    get_db_credentials()
    db_client = get_db_client(DATABASE)
    while True:
        ingest_submissions(args.spool_dir, trees_name, db_client)
        time.sleep(1)

if __name__ == "__main__":
    main()
