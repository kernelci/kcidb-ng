#!/usr/bin/env python3
"""
submissions_archiver.py
A script to archive submissions from a specified directory into a compressed tar file.
If storage token and URL are provided, it can also upload the archive to a remote server.

Config example (config.toml):
[submissions]
submissions_dir = "/path/to/submissions"
archive_dir = "/path/to/archive"
storage_token = "your_storage_token"
storage_url = "http://files.kernelci.org/upload"
storage_path = "/submissions"
"""

import os
import sys
import tarfile
import time
import shutil
import toml
import argparse
import requests


class Config:
    """
    Configuration class to load and access configuration settings.
    """

    def __init__(self, config_file):
        if not os.path.exists(config_file):
            print(f"Configuration file {config_file} does not exist")
            sys.exit(1)
        self.config = toml.load(config_file)

    def get(self, key, default=None):
        """
        Get a configuration value by key.
        """
        return self.config.get(key, default)


def archive_submissions(config):
    """
    Archive submissions in the submissions directory.
    """
    files = {}
    submissions_dir = config.get("submissions_dir", "/svc/kcidb-ng/spool/archive")
    archive_dir = config.get("archive_dir", "~/archive")
    print(f"Archiving submissions from {submissions_dir} to {archive_dir}")
    print(f"Submissions directory: {submissions_dir}")
    print(f"Archive directory: {archive_dir}")

    for file in os.listdir(submissions_dir):
        # array files{filename} = mtime
        files[file] = os.path.getmtime(os.path.join(submissions_dir, file))

    print(f"Found {len(files)} files to archive")
    # sort files by mtime
    files = sorted(files.items(), key=lambda x: x[1])
    archived_files = []

    archive_name = time.strftime("%Y%m%d%H%M%S")
    new_archive = os.path.join(archive_dir, archive_name + ".tar.xz")
    print(f"Creating archive {new_archive}")
    with tarfile.open(new_archive, "w:xz") as tar:
        print(f"Adding files to archive {new_archive}")
        for file, mtime in files:
            print(f"Adding file {file} to archive {new_archive}")
            tar.add(os.path.join(submissions_dir, file), arcname=file)
            archived_files.append(file)

    print(f"Archived {len(archived_files)} files to {new_archive}")

    # delete archived files from submissions directory
    for file in archived_files:
        os.remove(os.path.join(submissions_dir, file))

    return new_archive


def upload_file(file_path, config):
    """
    Upload the specified file to remote kernelci-storage.
    """
    storage_token = config.get("storage_token")
    storage_url = config.get("storage_url")
    storage_path = config.get("storage_path", "/submissions")

    if not storage_token or not storage_url:
        print("Storage token or URL not provided, skipping upload.")
        return

    print(f"Uploading {file_path} to {storage_url} with token {storage_token}")

    headers = {"Authorization": f"Bearer {storage_token}"}
    files = {"file0": open(file_path, "rb")}
    data = {"path": storage_path}

    response = requests.post(
        storage_url, headers=headers, files=files, data=data, timeout=60
    )
    if response.status_code != 200:
        print(
            f"Failed to upload file {file_path}: {response.status_code} {response.text}"
        )
        return
    else:
        print(f"File {file_path} uploaded successfully to {storage_url}")
        # unlink the file after upload
        os.unlink(file_path)


def main():
    """
    Main function.
    """
    arg_parser = argparse.ArgumentParser(description="Submissions Archiver/Uploader")
    arg_parser.add_argument(
        "--config",
        type=str,
        default="config.toml",
        help="Path to the configuration file",
    )
    args = arg_parser.parse_args()

    config = Config(args.config)
    archive_file = archive_submissions(config)
    print(f"Submissions archived to {archive_file}")
    if config.get("storage_token") and config.get("storage_url"):
        print("Uploading archive to remote storage...")
        upload_file(archive_file, config)


if __name__ == "__main__":
    main()
