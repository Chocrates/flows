"""In-process scheduler for periodic tasks.

Runs the overdue reminder sweep once a day at ``reminder_hour_utc``. Start
with ``python -m invoicing.jobs``.
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from typing import Callable

from invoicing import dunning
from invoicing.mailer import Mailer
from invoicing.settings import get_config
from invoicing.storage.db import open_database
from invoicing.storage.repository import CustomerRepository, InvoiceRepository

log = logging.getLogger(__name__)


@dataclass
class DailyTask:
    name: str
    hour_utc: int
    fn: Callable[[], object]
    last_run: datetime | None = field(default=None)

    def next_run(self, now: datetime) -> datetime:
        candidate = now.replace(hour=self.hour_utc, minute=0, second=0, microsecond=0)
        if candidate <= now:
            candidate += timedelta(days=1)
        return candidate


class Scheduler:
    def __init__(self) -> None:
        self.tasks: list[DailyTask] = []
        self._stop = threading.Event()

    def every_day(self, name: str, hour_utc: int, fn: Callable[[], object]) -> None:
        self.tasks.append(DailyTask(name, hour_utc, fn))

    def tick(self, now: datetime) -> None:
        for task in self.tasks:
            due = now.hour >= task.hour_utc
            ran_today = task.last_run is not None and task.last_run.date() == now.date()
            if due and not ran_today:
                log.info("running %s", task.name)
                try:
                    task.fn()
                except Exception:  # keep the loop alive
                    log.exception("task %s failed", task.name)
                task.last_run = now

    def run_forever(self, interval: float = 60.0) -> None:
        while not self._stop.is_set():
            self.tick(datetime.now(timezone.utc))
            time.sleep(interval)

    def stop(self) -> None:
        self._stop.set()


def build_scheduler() -> Scheduler:
    cfg = get_config()
    db = open_database(cfg.database_path)
    invoices, customers = InvoiceRepository(db), CustomerRepository(db)
    mailer = Mailer(cfg.smtp)
    sched = Scheduler()
    sched.every_day("reminders", cfg.reminder_hour_utc,
                    lambda: dunning.run_reminders(invoices, customers, mailer))
    return sched


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    build_scheduler().run_forever()
