#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-2.1-only

import json
import random
import string
from datetime import datetime, timezone

# Function to generate a random checkout ID
def generate_id():
    random_number = ''.join(random.choices(string.digits, k=6))
    return f"maestro:{random_number}"

# Function to generate a unique identifier for the file name
def generate_unique_identifier():
    return ''.join(random.choices(string.ascii_letters + string.digits, k=16))

# Generate the JSON structure
def generate_submission_json():
    now = datetime.now(timezone.utc).isoformat()
    checkout_id = generate_id()
    submission_data = {
        "checkouts": [
            {
                "id": checkout_id,
                "origin": "maestro",
                "git_repository_url": "https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git",
                "git_repository_branch": "master",
                "git_commit_hash": "9d946bf42e2fa2073df018d38fc8ff567b2ef061",
                "start_time": now
            }
        ],
        "builds": [
            {
                "checkout_id": checkout_id,
                "id": generate_id(),
                "origin": "maestro",
                "comment": "next-20250610",
                "start_time": now,
                "architecture": "arm64",
                "compiler": "gcc-12",
                "config_name": "defconfig+arm64-chromebook+CONFIG_RANDOMIZE_BASE=y",
                "status": "PASS",
                "misc": {
                    "platform": "kubernetes",
                    "runtime": "k8s-all",
                    "lab": "k8s-all",
                    "job_id": "kci-684137bc214050d34195376b-kbuild-gcc-12-arm64-rand-qlb46wr0",
                    "job_context": "gke_android-kernelci-external_europe-west4-c_kci-eu-west4",
                    "kernel_type": "image",
                    "maestro_viewer": "https://api.kernelci.org/viewer?node_id=684137bc214050d34195376b"
                },
                "output_files": [
                    {"name": "build_kimage_stderr_log", "url": "https://example.com/build_kimage_stderr.log.gz"},
                    {"name": "build_sh", "url": "https://example.com/build.sh"},
                    {"name": "build_dtbs_stderr_log", "url": "https://example.com/build_dtbs_stderr.log.gz"},
                    {"name": "build_kselftest_stderr_log", "url": "https://example.com/build_kselftest_stderr.log.gz"},
                    {"name": "build_modules_stderr_log", "url": "https://example.com/build_modules_stderr.log.gz"},
                    {"name": "build_dtbs_log", "url": "https://example.com/build_dtbs.log.gz"},
                    {"name": "build_kselftest_log", "url": "https://example.com/build_kselftest.log.gz"},
                    {"name": "build_kimage_log", "url": "https://example.com/build_kimage.log.gz"},
                    {"name": "build_modules_log", "url": "https://example.com/build_modules.log.gz"},
                    {"name": "metadata", "url": "https://example.com/metadata.json"}
                ],
                "config_url": "https://files.kernelci.org/kbuild-gcc-12-arm64-randomize-684137bc214050d34195376b/.config",
                "log_url": "https://files.kernelci.org/kbuild-gcc-12-arm64-randomize-684137bc214050d34195376b/build.log.gz",
                "log_excerpt": ""
            }
        ],
        "tests": [],
        "issues": [],
        "incidents": [],
        "version": {
            "major": 5,
            "minor": 3
        }
    }
    return submission_data

# Save the JSON to a file
def save_submission_json():
    submission_data = generate_submission_json()
    unique_identifier = generate_unique_identifier()
    file_name = f"submission-{unique_identifier}.json"
    with open(file_name, "w") as json_file:
        json.dump(submission_data, json_file, indent=2)
    print(f"Submission file saved as {file_name}")

if __name__ == "__main__":
    save_submission_json()