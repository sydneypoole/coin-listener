UPDATE users
SET password_hash = '$argon2id$v=19$m=19456,t=2,p=1$c29tZXJhbmRvbXNhbHQ$laqOUbdkJho4NACYmDwyLQdS/qq83rIuReZa+IyST2I',
    updated_at = NOW()
WHERE email = 'admin@example.com'
  AND password_hash = 'admin';

INSERT INTO schema_migrations_marker (name)
VALUES ('0012_auth_session_baseline')
ON CONFLICT (name) DO NOTHING;
