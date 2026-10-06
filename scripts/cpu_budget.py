"""Affinity plus minimum visible ancestor cgroup v1/v2 CPU quota, rounded up."""
import math
import os
from pathlib import Path


def available_cpus():
    try:
        count = len(os.sched_getaffinity(0))
        for line in Path('/proc/self/cgroup').read_text().splitlines():
            _, controllers, relative = line.split(':', 2)
            base = Path('/sys/fs/cgroup')
            if controllers:
                if 'cpu' not in controllers.split(','):
                    continue
                base /= 'cpu'
            current = base / relative.lstrip('/')
            # Include root even when a namespace hides the process's leaf path.
            while current == base or base in current.parents:
                try:
                    if controllers:
                        quota = int((current / 'cpu.cfs_quota_us').read_text())
                        period = int((current / 'cpu.cfs_period_us').read_text())
                    else:
                        quota_text, period_text = (current / 'cpu.max').read_text().split()
                        quota = -1 if quota_text == 'max' else int(quota_text)
                        period = int(period_text)
                    if quota > 0 and period > 0:
                        count = min(count, math.ceil(quota / period))
                except (OSError, ValueError):
                    pass
                if current == base:
                    break
                current = current.parent
        return max(1, count)
    except (OSError, ValueError, AttributeError):
        return 1


def build_jobs():
    return max(1, available_cpus() // 2)
