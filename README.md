# updown

Your friendly neighborhood uptime monitor

### quickstart

```sh
git clone https://github.com/swlkr/updown.git
cd updown
cp .env.example .env
export $(cat .env | xargs)
cargo run
# wait a while for cargo to download and compile the crates, there's a lot of them
# open localhost:9001
```

### stack

- tailwindcss
- dioxus liveview
- salvo
- sqlite
- rust

### overview

It starts out a regular old ssr app without js/wasm but quickly escalates into a dioxus liveview app after you login.
The auth is a little weird but you'll be right at home if you've tried mullvad vpn's signup.
Instead of taking an email / password or some oauth thing, the app gives you a 16 digit login code (which isn't shown, so you'll have to check sqlite if you want to login again).
This allows you to signup with your username and get logged in all in the same step, no emails, no passwords, just that sweet, sweet login code.
Yes, if you forget this login code, you will not be able to log in again, which is a downside.

### files

| name | description |
| --- | --- |
| main.rs | the routes, the database and the dioxus components all kind of mangled together |

### routes

| method | route        | fn               | rendered | description                                                   |
| --- | --- | --- | --- | --- |
| GET    | /            | index()          | ssr      | the landing page with sign up form                            |
| GET    | /login       | new_session()    | ssr      | the page with the login form                                  |
| POST   | /login       | add_session()    | ssr      | exactly what it sounds like                                   |
| POST   | /logout      | del_session()    | ssr      | the page with the list of configured linkks                   |
| GET    | /            | index()          | ssr      | this is where the dioxus liveview app sets #main if authed    |
| GET    | /ws          | liveview()       | liveview | the actual websocket connection to initialize liveview        |

### major ui components

| name | description |
| --- | --- |
| Root | the entry point to the dioxus liveview app            |
| SiteList | the list of sites, either in edit                 |
| Nav | the nav at the top or bottom if the viewport is mobile |

That's pretty much it, happy hacking!
