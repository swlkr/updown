create table if not exists users (
    id integer not null primary key,
    login_code text not null unique,
    created_at integer not null,
    updated_at integer not null
);