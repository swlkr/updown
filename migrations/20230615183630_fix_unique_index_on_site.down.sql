pragma foreign_keys=off;

drop index if exists unique_url_user_id;

create table sites_new (
    id integer not null primary key,
    name text,
    url text not null unique,
    updated_at integer not null,
    created_at integer not null,
    user_id integer not null references users(id)
);

insert into sites_new select * from sites;

drop table sites;

alter table sites_new rename to sites;

pragma foreign_keys=on;