---
title: Docker
description: The containers of the project you are in — state, ports, logs, and the usual compose commands.
---

In a project with a compose file, a Dockerfile, or containers of its own, the status bar shows how many of its containers are running (🐳 3 running). Click it for the panel.

Agentty looks for `compose.yaml`, `compose.yml`, `docker-compose.yml` and `docker-compose.yaml` at the project root — the same names and the same order `docker compose` itself uses. Without a compose file, it lists the project's own containers instead.

The panel follows the working tree of the active pane, so switching to a pane in another project switches the containers you see.

## What the panel shows

For each compose service or container: its image, state (running, stopped, unhealthy) and ports.

## What you can do

| Action | Note |
|---|---|
| Start / stop / restart a service | |
| `compose up -d` | May pull images first, so it is allowed to take a while |
| `compose down` | **Asked twice.** Volumes are kept |
| Logs | Opens in a new tab, so you can keep it while you work |

## What it deliberately does not read

**Environment values are never read or shown.** Compose files and the `.env` files beside them routinely hold database passwords, and `docker compose config` would print them fully resolved. Agentty reads only names, images, states and ports.

Every Docker command is run as an argument list, never through a shell, and a service or container name is only ever passed after Docker itself reported it and the name passed a validity check. A malformed name in a compose file cannot turn into a command.

## If the panel is empty

- The Docker daemon may still be starting — listing has a 15 second timeout so a slow daemon can't freeze the panel.
- Check that the project actually has one of the four compose file names at its root, or containers labeled for it.
