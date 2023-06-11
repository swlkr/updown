async function login(login_code) {
    const response = await fetch("/login", {
        method: "POST",
        headers: {
            "Accept": "application/json",
            "Content-Type": "application/json"
        },
        body: JSON.stringify({ login_code: login_code })
    });
    if(response.ok) {
        window.location.reload();
    }
}

async function signup(url) {
    const response = await fetch("/signup", {
        method: "POST",
        headers: {
            "Accept": "application/json",
            "Content-Type": "application/json"
        },
        body: JSON.stringify({ url: url })
    });
    try {
        await response.json();
        window.location.reload();
    } catch(error) {
    }
}

async function logout() {
    const response = await fetch("/logout", {
        method: "POST",
        headers: {
            "Content-Type": "application/json"
        }
    });
    try {
        await response.json();
        window.location.reload();
    } catch(error) {}
}

document.addEventListener("click", (event) => {
    if(event.target.id === "login-btn") {
        const login_code = document.querySelector('input[name="login-code"]').value;
        if(login_code.length !== 21) { return; }
        login(login_code).then(x => x);
    }
    if(event.target.id === "signup-btn") {
        const url = document.querySelector('input[name="url"]').value;
        signup(url).then(x => x);
    }
    if(event.target.id === "logout-btn") {
        logout().then(x => x);
    }
});
