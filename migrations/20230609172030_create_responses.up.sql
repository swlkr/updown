create table responses (
    id integer not null primary key,
    site_id integer not null references sites(id),
    status_code integer not null,
    created_at integer not null,
    updated_at integer not null,
    unique(site_id, status_code)
)