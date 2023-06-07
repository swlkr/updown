create table if not exists logins (
    id integer not null primary key,
    user_id integer not null references users(id),
    created_at integer not null
)