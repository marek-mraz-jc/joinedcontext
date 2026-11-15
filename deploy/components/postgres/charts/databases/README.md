# Databases Helm Chart

This Helm chart manages PostgreSQL databases and their associated roles in a CloudNativePG cluster.

## Features

- Create and manage databases using CloudNativePG Database CRDs
- Automatically create additional database roles with configurable permissions
- Support for read-only and read/write role types
- Database-scoped permissions (roles have no access to other databases)

## Configuration

### Basic Database Configuration

```yaml
cluster:
  name: postgres-cluster

databases:
  myapp:
    name: 'myapp'
    user: 'myapp'
```

### Adding Additional Roles

You can add additional roles to any database using the `additionalRoles` list. Each role can be configured as either `readonly` or `readwrite`.

```yaml
databases:
  myapp:
    name: 'myapp'
    user: 'myapp'
    additionalRoles:
      - name: 'myapp-readonly'
        type: 'readonly'
        credentialsSecret:
          name: 'myapp-readonly-credentials'
          passwordKey: 'password'

      - name: 'myapp-analytics'
        type: 'readwrite'
        credentialsSecret:
          name: 'myapp-analytics-credentials'
          passwordKey: 'db-password'
```

### Role Configuration Options

| Field                           | Required | Default    | Description                                    |
| ------------------------------- | -------- | ---------- | ---------------------------------------------- |
| `name`                          | Yes      | -          | The PostgreSQL role name                       |
| `type`                          | No       | `readonly` | Permission type: `readonly`, `readwrite` or `login` |
| `credentialsSecret.name`        | Yes      | -          | Kubernetes secret containing the role password |
| `credentialsSecret.passwordKey` | No       | `password` | Key name in the secret for the password        |

### Permission Types

#### `readonly`

- `SELECT` on all tables
- `SELECT` on all sequences
- `CONNECT` on the database
- `USAGE` on the public schema
- Default privileges for future objects

#### `login`

- `CONNECT` on the database, nothing else: the program's own migrations grant the role its tables
  (jc's shako, daiya, dataroom and ekiben request roles), so no upgrade revokes them

#### `readwrite`

- `SELECT`, `INSERT`, `UPDATE`, `DELETE` on all tables
- `USAGE`, `SELECT`, `UPDATE` on all sequences
- `EXECUTE` on all functions
- `CONNECT` on the database
- `USAGE` on the public schema
- Default privileges for future objects

## How It Works

When you configure additional roles, the chart creates Kubernetes Jobs that:

1. Connect to the specified database using the cluster admin credentials
2. Create or update the role with the provided credentials
3. Revoke all existing privileges to ensure clean state
4. Grant appropriate permissions based on the role type
5. Set default privileges for future database objects
6. Verify the role has no system-level privileges

### Job Details

- Jobs are created as Helm hooks (`post-install`, `post-upgrade`)
- Jobs run with `hook-delete-policy: before-hook-creation`
- Completed jobs are automatically deleted after 300 seconds (TTLSecondsAfterFinished)
- Jobs will retry up to 5 times on failure (backoffLimit)

## Security Considerations

- Roles are created with **no superuser privileges**
- Roles are scoped to a **single database only**
- Roles cannot create other roles or databases
- You must provide credentials via Kubernetes secrets
- The chart uses the cluster's app credentials for administrative operations

## Example Secret Creation

Before deploying a database with additional roles, create the required secrets:

```bash
kubectl create secret generic myapp-readonly-credentials \
  --from-literal=password='your-secure-password'
```

Or using a YAML manifest:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: myapp-readonly-credentials
type: Opaque
stringData:
  password: 'your-secure-password'
```

## Limitations

- Roles are currently scoped to the `public` schema only
- The chart assumes the cluster admin secret is named `{cluster.name}-superuser`
- The chart connects to the read-write service endpoint: `{cluster.name}-rw`
