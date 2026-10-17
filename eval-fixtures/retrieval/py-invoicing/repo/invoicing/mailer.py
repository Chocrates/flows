"""Outbound email over SMTP."""

from __future__ import annotations

import logging
import smtplib
from dataclasses import dataclass, field
from email.message import EmailMessage

from invoicing.settings import SmtpConfig, get_config

log = logging.getLogger(__name__)


@dataclass
class Outgoing:
    to: str
    subject: str
    body: str
    attachments: list[tuple[str, bytes, str]] = field(default_factory=list)


def build_message(msg: Outgoing, sender: str) -> EmailMessage:
    em = EmailMessage()
    em["From"] = sender
    em["To"] = msg.to
    em["Subject"] = msg.subject
    em.set_content(msg.body)
    for filename, data, mime in msg.attachments:
        maintype, subtype = mime.split("/", 1)
        em.add_attachment(data, maintype=maintype, subtype=subtype, filename=filename)
    return em


class Mailer:
    def __init__(self, cfg: SmtpConfig | None = None) -> None:
        self.cfg = cfg or get_config().smtp

    def send(self, msg: Outgoing) -> bool:
        """Send one message. Returns False (and logs) on SMTP failure."""
        em = build_message(msg, self.cfg.sender)
        try:
            with smtplib.SMTP(self.cfg.host, self.cfg.port, timeout=30) as smtp:
                if self.cfg.use_tls:
                    smtp.starttls()
                if self.cfg.username:
                    smtp.login(self.cfg.username, self.cfg.password or "")
                smtp.send_message(em)
        except (smtplib.SMTPException, OSError) as exc:
            log.warning("mail to %s failed: %s", msg.to, exc)
            return False
        return True


class MemoryMailer(Mailer):
    """Collects messages instead of sending; used in tests."""

    def __init__(self) -> None:
        self.sent: list[Outgoing] = []

    def send(self, msg: Outgoing) -> bool:
        self.sent.append(msg)
        return True
