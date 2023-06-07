create table if not exists sites (
    id integer not null primary key,
    name text,
    url text not null unique,
    updated_at integer not null,
    created_at integer not null,
    user_id integer not null references users(id)
);