# Running Quiczilla as a background receiver

The daemon is a foreground process. These examples provide a small, auditable
wrapper for automatic startup; they do not change the transfer protocol.

## Linux (systemd)

1. Install the release binaries so `quic` and `quiczilla-worker` are available
   at `/usr/local/bin/`.
2. Create the service account and directories:

   ```sh
   sudo useradd --system --home /var/lib/quiczilla --create-home --shell /usr/sbin/nologin quiczilla
   sudo install -d -o quiczilla -g quiczilla /srv/quiczilla /var/lib/quiczilla/identity /etc/quiczilla
   sudo install -o quiczilla -g quiczilla -m 0640 authorized_thumbprints /etc/quiczilla/authorized_thumbprints
   ```

3. Review `quiczilla.service`, copy it into `/etc/systemd/system/`, then start it:

   ```sh
   sudo install -m 0644 deploy/quiczilla.service /etc/systemd/system/quiczilla.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now quiczilla
   sudo systemctl status quiczilla
   sudo journalctl -u quiczilla -f
   ```

   Allow UDP 55441 in the host/network firewall when clients are not on a
   private mesh. The template uses `--on-conflict refuse`; remove that option if
   compatibility with overwrite-on-retry is preferred.

## Windows (Task Scheduler)

Quiczilla does not currently register a native Windows Service. Use Task
Scheduler for unattended startup, or run `quic daemon ...` in a supervised
terminal. Create a task whose action is the installed `quic.exe` with arguments
such as:

```text
daemon --allow-thumbprints C:\ProgramData\Quiczilla\authorized_thumbprints --save-dir C:\Quiczilla\Incoming --identity-dir C:\ProgramData\Quiczilla\identity --on-conflict refuse --port 55441
```

Set the task to run whether the user is logged on or not, restart on failure,
and grant its account write access to the save and identity directories. Allow
UDP 55441 through Windows Firewall. A native Windows Service wrapper can be
added later without changing this command line.
