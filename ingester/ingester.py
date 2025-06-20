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
from concurrent.futures import ProcessPoolExecutor
import functools
import requests
import hashlib
import tempfile

# default database
DATABASE = "postgresql:dbname=kcidb user=kcidb password=kcidb host=localhost port=5432"
VERBOSE = 0
STORAGE_TOKEN = os.environ.get("STORAGE_TOKEN", None)
LOGEXCERPT_THRESHOLD = 256  # 256 bytes threshold for logexcerpt

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


def upload_logexcerpt(logexcerpt, id):
    """
    Upload logexcerpt to storage and return a reference(URL)
    """
    STORAGE_BASE_URL = "https://files-staging.kernelci.org"
    upload_url = f"{STORAGE_BASE_URL}/upload"
    if VERBOSE:
        logger.info(f"Uploading logexcerpt for {id} to {upload_url}")
    # make temporary file with logexcerpt data
    with tempfile.NamedTemporaryFile(delete=False, suffix=".logexcerpt") as temp_file:
        logexcerpt_filename = temp_file.name
        temp_file.write(logexcerpt.encode('utf-8'))
        temp_file.flush()
    with open(logexcerpt_filename, "rb") as f:
        hdr = {
            "Authorization": f"Bearer {STORAGE_TOKEN}",
        }
        files={
            "file0": ("logexcerpt.txt", f),
            "path": f"logexcerpt/{id}"
        }
        try:
            r = requests.post(
                upload_url,
                headers=hdr,
                files=files
            )
        except Exception as e:
            logger.error(f"Error uploading logexcerpt for {id}: {e}")
            os.remove(logexcerpt_filename)
            return logexcerpt  # Return original logexcerpt if upload fails
    os.remove(logexcerpt_filename)
    if r.status_code != 200:
        logger.error(f"Failed to upload logexcerpt for {id}: {r.status_code} : {r.text}")
        return logexcerpt  # Return original logexcerpt if upload fails

    return f"{STORAGE_BASE_URL}/logexcerpt/{id}/logexcerpt.txt"


def extract_log_excerpt(input_data):
    """
    Extract log_excerpt from builds and tests, if it is large,
    upload to storage and replace with a reference
    """
    if not STORAGE_TOKEN:
        logger.warning("STORAGE_TOKEN is not set, log_excerpts will not be uploaded")
        return input_data

    builds = input_data.get("builds", [])
    tests = input_data.get("tests", [])
    for build in builds:
        if build.get("log_excerpt"):
            id = build.get("id", "unknown")
            log_excerpt = build["log_excerpt"]
            if isinstance(log_excerpt, str) and len(log_excerpt) > LOGEXCERPT_THRESHOLD:
                log_hash = hashlib.sha256(log_excerpt.encode('utf-8')).hexdigest()
                if VERBOSE:
                    logger.info(f"Uploading log_excerpt for build {id} hash {log_hash} with size {len(log_excerpt)} bytes")
                # Upload to storage and replace with a reference
                build["log_excerpt"] = upload_logexcerpt(log_excerpt, log_hash)

    for test in tests:
        if test.get("log_excerpt"):
            id = test.get("id", "unknown")
            log_excerpt = test["log_excerpt"]
            if isinstance(log_excerpt, str) and len(log_excerpt) > LOGEXCERPT_THRESHOLD:
                log_hash = hashlib.sha256(log_excerpt.encode('utf-8')).hexdigest()
                if VERBOSE:
                    logger.info(f"Uploading log_excerpt for test {id} hash {log_hash} with size {len(log_excerpt)} bytes")
                # Upload to storage and replace with a reference
                test["log_excerpt"] = upload_logexcerpt(log_excerpt, log_hash)
    return input_data

def process_file(filename, trees_name, db_client, spool_dir):
    """ Process a single file, standardizing tree names and loading into the database """
    failed_dir = os.path.join(spool_dir, "failed")
    archive_dir = os.path.join(spool_dir, "archive")
 
    full_filename = os.path.join(spool_dir, filename)
    with open(full_filename, "r") as f:
        fsize = os.path.getsize(full_filename)
        if fsize == 0:
            if VERBOSE:
                logger.info(f"File {full_filename} is empty, skipping, deleting")
            os.remove(full_filename)
            return False
        start_time = time.time()
        if VERBOSE:
            logger.info(f"File size: {fsize}")
        try:
            data = json.loads(f.read())
            data = extract_log_excerpt(data)
            data = standardize_trees_name(data, trees_name)
            io_schema = db_client.get_schema()[1]
            data = io_schema.validate(data)
            data = io_schema.upgrade(data, copy=False)
            db_client.load(data)
        except Exception as e:
            logger.error(f"Error loading data: {e}")
            logger.error(f"File: {full_filename}")
            move_file_to_failed_dir(full_filename, failed_dir)
            return False
        ing_speed = fsize / (time.time() - start_time) / 1024
        if VERBOSE:
            logger.info(f"Ingested {filename} in {ing_speed} KB/s")
        # Archive the file
        try:
            os.rename(full_filename, os.path.join(archive_dir, filename))
        except Exception as e:
            logger.error(f"Error archiving file {filename}: {e}")
            return False

    return True


def ingest_submissions(spool_dir, trees_name, db_client=None):
    if db_client is None:
        raise Exception("db_client is None")
    io_schema = db_client.get_schema()[1]
    # iterate over all files in the directory spool_dir
    stat_ok = 0
    stat_fail = 0
    for filename in os.listdir(spool_dir):
        # skip directories
        if os.path.isdir(os.path.join(spool_dir, filename)):
            continue
        # skip if not json
        if not filename.endswith(".json"):
            continue
        logger.info(f"Ingesting {filename}")
        r = process_file(filename, trees_name, db_client, spool_dir)
        if r:
            stat_ok += 1
        else:
            stat_fail += 1
    # iterate over res and print statistics
    # this also will wait asynchronously for all files to be processed
    if stat_ok + stat_fail > 0:
        logger.info(f"Processed {stat_ok + stat_fail} files: {stat_ok} succeeded, {stat_fail} failed")


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
