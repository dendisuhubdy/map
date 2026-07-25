# Droplet Runbook

Host provisioning for the NL trip-planner map. Executed 2026-07-25.

## Current host

| | |
|---|---|
| Name | `map-sgp1` |
| Droplet ID | `587451535` |
| IP | `159.65.9.170` |
| Region | `sgp1` (Singapore — closest DO region to Indonesia) |
| Size | `s-8vcpu-16gb` — 16 GB RAM, 8 vCPU, 320 GB SSD, $96/mo |
| Image | `ubuntu-24-04-x64` |
| SSH keys | `peptidebay-do` (57459247), `mbp-access` (55402969) |
| Tag | `scenicmap` |

SSH alias is in `~/.ssh/config` on the workstation:

```
Host map-sgp1
    HostName 159.65.9.170
    User root
    IdentityFile ~/.ssh/id_ed25519
```

So: `ssh map-sgp1`.

## Deviation from the design spec: no block storage

The spec (decision 2) budgeted a 100 GB block volume at $10/mo, assuming a small root
disk. `s-8vcpu-16gb` ships **320 GB of local SSD**, which is 14× the ~22 GB of artifacts,
so `/data` is a plain directory on the root disk and no volume was created.

Rationale: everything under `/data` is reproducible from public sources via `make all`.
Losing it to a droplet rebuild costs a few hours of re-import, not any unique state. Local
NVMe is also faster for the I/O-heavy import steps.

Reversible without a rebuild if the tradeoff ever changes:

```bash
doctl compute volume create map-data --region sgp1 --size 100GiB \
  --access-token "$DIGITAL_OCEAN_API_TOKEN_PERSONAL"
doctl compute volume-action attach <VOLUME_ID> 587451535 \
  --access-token "$DIGITAL_OCEAN_API_TOKEN_PERSONAL"
# then: mkfs.ext4, mount at /mnt/data, rsync -a /data/ /mnt/data/, swap the mountpoint
```

## Provisioning steps

Creation:

```bash
doctl compute droplet create map-sgp1 \
  --access-token "$DIGITAL_OCEAN_API_TOKEN_PERSONAL" \
  --region sgp1 --size s-8vcpu-16gb --image ubuntu-24-04-x64 \
  --ssh-keys 57459247,55402969 --enable-monitoring --tag-name scenicmap --wait
```

Then over SSH, as root:

```bash
# Fresh droplets run cloud-init's own apt; racing it fails with a lock error.
cloud-init status --wait

mkdir -p /data && chmod 755 /data

APT="apt-get -o DPkg::Lock::Timeout=600 -qq"
$APT update
$APT install -y curl jq pbzip2 make git ca-certificates netcat-openbsd bc

curl -fsSL https://get.docker.com | sh
systemctl enable --now docker

# Allow OpenSSH BEFORE enabling, or you lock yourself out.
ufw allow OpenSSH && ufw allow 80/tcp && ufw allow 443/tcp && ufw --force enable

fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
echo '/swapfile none swap sw 0 0' >> /etc/fstab
```

Verified state: Docker 29.6.2, Compose 5.3.1, 15 GiB RAM, 8 CPUs, 4 GiB swap,
303 G free on `/data`, ufw active.

## Deploy loop

```bash
# workstation
git push origin <branch>

# droplet
ssh map-sgp1 'cd /opt/map && git pull && docker compose up -d --build'
```

Repo lives at `/opt/map` on the droplet. `.env` is **not** in git — it is created on the
droplet from `.env.example` and holds the Postgres password and `SITE_HOST`.

## Teardown

```bash
doctl compute droplet delete map-sgp1 --access-token "$DIGITAL_OCEAN_API_TOKEN_PERSONAL"
```
