# Worker SSH Setup and Hardening

This document describes the SSH setup for RCH Xcode Lane workers. Following these steps ensures secure, restricted access suitable for CI execution environments.

## Security Rationale

Workers execute arbitrary Xcode build phases from repository code, so they should be treated as CI machines. The SSH configuration minimizes attack surface by:

- Using a dedicated user with no interactive shell access
- Restricting SSH to JSON RPC over stdin/stdout only
- Disabling unnecessary SSH features (agent forwarding, PTY allocation)
- Pinning host keys to prevent TOFU (Trust On First Use) surprises

## Prerequisites

- macOS worker with administrator access for initial setup
- Host machine with SSH client
- `rch-xcode` binary built and available

## Worker Setup Steps

### 1. Create Dedicated User

Create a dedicated `rch` user on the worker:

```bash
# On the worker (requires admin)
sudo dscl . -create /Users/rch
sudo dscl . -create /Users/rch UserShell /usr/bin/false
sudo dscl . -create /Users/rch RealName "RCH Worker"
sudo dscl . -create /Users/rch UniqueID 601  # Choose an unused UID
sudo dscl . -create /Users/rch PrimaryGroupID 20  # staff group
sudo dscl . -create /Users/rch NFSHomeDirectory /Users/rch
sudo mkdir -p /Users/rch
sudo chown rch:staff /Users/rch
```

### 2. Create SSH Directory

```bash
sudo mkdir -p /Users/rch/.ssh
sudo chmod 700 /Users/rch/.ssh
sudo chown rch:staff /Users/rch/.ssh
```

### 3. Generate Key Pair (on host)

Generate a dedicated key pair for RCH operations on the host machine:

```bash
# On the host
ssh-keygen -t ed25519 -f ~/.ssh/rch_worker_key -C "rch-xcode-lane"
```

### 4. Configure Forced Command

Create the worker RPC handler script on the worker:

```bash
# On the worker
sudo tee /Users/rch/rch-worker-entrypoint.sh > /dev/null << 'EOF'
#!/bin/bash
# RCH Worker Entrypoint - forced command for SSH
# Only accepts JSON RPC over stdin/stdout

# Log for audit (optional)
logger -t rch-worker "Connection from ${SSH_CLIENT%% *}"

# Execute the worker RPC handler
exec /usr/local/bin/rch-xcode worker serve
EOF

sudo chmod 755 /Users/rch/rch-worker-entrypoint.sh
sudo chown rch:staff /Users/rch/rch-worker-entrypoint.sh
```

### 5. Configure Authorized Keys with Restrictions

Add the public key with forced command and restrictions:

```bash
# On the worker
sudo tee /Users/rch/.ssh/authorized_keys > /dev/null << EOF
restrict,command="/Users/rch/rch-worker-entrypoint.sh" $(cat ~/.ssh/rch_worker_key.pub)
EOF

sudo chmod 600 /Users/rch/.ssh/authorized_keys
sudo chown rch:staff /Users/rch/.ssh/authorized_keys
```

The `restrict` option disables:
- Agent forwarding
- Port forwarding
- PTY allocation
- X11 forwarding
- User rc file execution

### 6. Optional: Restrict Source Addresses

For additional security, restrict connections to specific IP ranges:

```bash
# Add source address restriction to authorized_keys
# Replace 10.0.0.0/8 with your actual host network
sudo tee /Users/rch/.ssh/authorized_keys > /dev/null << EOF
restrict,command="/Users/rch/rch-worker-entrypoint.sh",from="10.0.0.0/8,192.168.0.0/16" $(cat ~/.ssh/rch_worker_key.pub)
EOF
```

## Host Configuration

### 1. SSH Config Entry

Add the worker to the host's SSH config:

```bash
# ~/.ssh/config
Host rch-worker-1
    HostName worker1.example.com
    User rch
    IdentityFile ~/.ssh/rch_worker_key
    IdentitiesOnly yes
    # Disable unnecessary features
    ForwardAgent no
    ForwardX11 no
    RequestTTY no
    # Connection settings
    ConnectTimeout 30
    ServerAliveInterval 15
    ServerAliveCountMax 3
```

### 2. Pin Worker Host Keys

After first successful connection, pin the host key to prevent TOFU attacks:

```bash
# Get the worker's host key
ssh-keyscan -t ed25519 worker1.example.com >> ~/.ssh/known_hosts

# Or manually verify and add
ssh-keygen -F worker1.example.com
```

For programmatic pinning in workers.toml:

```toml
[[workers]]
name = "mac-mini-1"
host = "worker1.example.com"
user = "rch"
identity_file = "~/.ssh/rch_worker_key"
host_key_fingerprint = "SHA256:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```

## Verification

### Test Connection

```bash
# Should output JSON RPC capabilities (probe response)
echo '{"protocol_version":0,"request_id":"test","op":"probe"}' | \
  ssh rch-worker-1 2>/dev/null | jq .
```

Expected output includes `capabilities` with `protocol_version`, `operations`, etc.

### Verify Restrictions

```bash
# These should all fail (no shell, no port forwarding)
ssh rch-worker-1 whoami          # Should fail or return forced command output
ssh -N -L 8080:localhost:80 rch-worker-1  # Should fail
```

## Troubleshooting

### Connection Refused

1. Verify SSH service is running on worker: `sudo systemsetup -getremotelogin`
2. Check firewall settings allow SSH port
3. Verify the rch user exists and SSH directory permissions

### Permission Denied

1. Verify public key is in authorized_keys
2. Check authorized_keys permissions (600)
3. Check .ssh directory permissions (700)
4. Check user home directory permissions

### Command Not Found

1. Verify rch-xcode binary is installed at the correct path
2. Check entrypoint script permissions (755)
3. Verify PATH includes binary location

## Security Checklist

- [ ] Dedicated `rch` user created with `/usr/bin/false` shell
- [ ] SSH key pair generated with no passphrase (or use ssh-agent)
- [ ] `restrict` option used in authorized_keys
- [ ] `command=` forced command restricts to RPC handler only
- [ ] Agent forwarding disabled
- [ ] PTY allocation disabled
- [ ] Host key pinned in known_hosts or workers.toml
- [ ] Source address restrictions applied (if applicable)
- [ ] Worker binary installed and executable
- [ ] Connection tested with probe request
