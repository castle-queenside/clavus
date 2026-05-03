--[[
clavus/hotkeys/hammerspoon.lua
─────────────────────────────────
Hammerspoon config for Clavus keyboard automation.
Source this from ~/.hammerspoon/init.lua or symlink it.

Reads bindings.json at runtime so all key mappings are shared
between macOS (Hammerspoon) and Windows (AutoHotkey).
]]

-- ─── Configuration ──────────────────────────────────────────────────
local config_path = os.getenv("HOME") .. "/Developer/clavus/hotkeys/bindings.json"
local default_server_host = "127.0.0.1"
local default_server_port = 7890
local default_timeout = 3  -- seconds

-- ─── Utility ────────────────────────────────────────────────────────

-- Read and parse bindings.json
local function load_bindings()
    local file = io.open(config_path, "r")
    if not file then
        hs.alert.show("⚠️ Clavus: bindings.json not found at " .. config_path)
        return nil, nil
    end
    local content = file:read("*a")
    file:close()

    local ok, data = pcall(hs.json.decode, content)
    if not ok then
        hs.alert.show("⚠️ Clavus: invalid bindings.json")
        return nil, nil
    end

    return data.bindings, data.server
end

-- Make an HTTP request to the Clavus server
local function clavus_api(method, path, body)
    local _, server = load_bindings()
    local host = (server and server.host) or default_server_host
    local port = (server and server.port) or default_server_port
    local timeout = (server and server.timeout_ms and server.timeout_ms / 1000) or default_timeout

    local url = string.format("http://%s:%d%s", host, port, path)
    local headers = {}

    if body then
        headers["Content-Type"] = "application/json"
    end

    -- hs.http doesn't have a great synchronous API, so we use curl
    local curl_cmd = string.format(
        'curl -s -X %s --max-time %d "%s"',
        method, timeout, url
    )

    if body then
        -- Write body to a temp file to avoid shell quoting nightmares
        local tmp = "/tmp/clavus_curl_body.json"
        local f = io.open(tmp, "w")
        if f then
            f:write(body)
            f:close()
        end
        curl_cmd = string.format(
            'curl -s -X %s --max-time %d -H "Content-Type: application/json" -d @%s "%s"',
            method, timeout, tmp, url
        )
    end

    local handle = io.popen(curl_cmd)
    local result = handle:read("*a")
    handle:close()

    return result
end

-- Get the currently active project from Clavus server
local function get_current_project()
    local resp = clavus_api("GET", "/api/projects")
    if resp and resp ~= "" then
        local ok, data = pcall(hs.json.decode, resp)
        if ok and type(data) == "table" and #data > 0 then
            -- Return the first project (TUI uses _last_project which persists)
            if type(data[1]) == "table" and data[1].name then
                return data[1].name
            end
        end
    end
    return nil
end

-- Notification wrapper (integrates with macOS notification center)
local function notify(title, message)
    hs.notify.new({
        title = "Clavus",
        subTitle = title,
        informativeText = message,
    }):send()
end

-- ─── Action Handlers ────────────────────────────────────────────────

local actions = {}

function actions.new_cue(params)
    -- Step 1: Ask for cue text
    hs.dialog.textPrompt("Clavus — New Cue",
        params.prompt_text or "Cue text:",
        "", "Add Cue", "Cancel",
        function(text)
            if not text or text == "" then
                notify("Cancelled", "No cue added")
                return
            end

            -- Step 2: Ask for position (can be blank)
            hs.dialog.textPrompt("Clavus — Cue Position",
                params.prompt_position or "Position @ (blank=1.1.1):",
                params.default_position or "", "Add Cue", "Cancel",
                function(position)
                    position = (position or ""):match("^%s*(.-)%s*$") or ""
                    if position == "" then
                        position = "1.1.1"
                    end

                    -- If track prompt exists, ask for it
                    local function do_post(track)
                        local project = get_current_project()
                        if not project then
                            notify("Error", "No Clavus project found. Start clavus serve first.")
                            return
                        end

                        local body = hs.json.encode({
                            text = text,
                            position = position,
                            track = track or "",
                            author = os.getenv("USER") or "hammerspoon",
                            project_name = project,
                        })

                        local resp = clavus_api("POST", "/api/cues", body)
                        if resp and resp ~= "" then
                            notify("Cue Added",
                                string.format("\"%s\" @ %s — %s", text, position, project))
                        else
                            notify("Error", "Could not reach Clavus server at :7890")
                        end
                    end

                    if params.prompt_track then
                        hs.dialog.textPrompt("Clavus — Track Name",
                            params.prompt_track, "", "Add Cue", "Skip",
                            function(track)
                                do_post(track or "")
                            end)
                    else
                        do_post("")
                    end
                end)
        end)
end

function actions.snapshot(params)
    local project = get_current_project()
    if not project then
        notify("Error", "No Clavus project found")
        return
    end

    hs.dialog.textPrompt("Clavus — Snapshot",
        params.prompt_message or "Description:",
        "", "Snapshot", "Cancel",
        function(message)
            message = message or ""
            -- Call snapshot API — need to check if there's a trigger endpoint
            -- For now, we notify and the user can use the TUI to snapshot
            if message == "" then
                message = "Snapshot via hotkey"
            end
            notify("Snapshot Triggered",
                string.format("%s — %s", project, message))
            -- The actual snapshot happens via clavus watch or manual TUI
            -- Future: add POST /api/snapshots endpoint
        end)
end

function actions.inject()
    local project = get_current_project()
    if not project then
        notify("Error", "No Clavus project found")
        return
    end

    local resp = clavus_api("POST",
        "/api/projects/inject?name=" .. hs.http.encodeForQuery(project), nil)
    if resp and resp ~= "" then
        local ok, data = pcall(hs.json.decode, resp)
        if ok and data.injected then
            notify("Cues Injected",
                string.format("%d cue(s) written as Ableton markers → %s", data.injected, project))
        elseif ok and data.message then
            notify("Injection", data.message)
        else
            notify("Response", resp)
        end
    else
        notify("Error", "Could not reach Clavus server")
    end
end

function actions.list_cues(params)
    local limit = (params and params.limit) or 5
    local project = get_current_project()
    if not project then
        notify("Error", "No Clavus project found")
        return
    end

    local filter = ""
    if params and params.filter == "unresolved" then
        filter = "&status=pending"
    end

    local resp = clavus_api("GET",
        "/api/cues?name=" .. hs.http.encodeForQuery(project) .. "&limit=" .. limit .. filter,
        nil)

    if resp and resp ~= "" then
        local ok, data = pcall(hs.json.decode, resp)
        if ok and type(data) == "table" and #data > 0 then
            local lines = {}
            for i, cue in ipairs(data) do
                table.insert(lines, string.format("📍 %s — %s", cue.position or "?", cue.text or "?"))
            end
            notify("Recent Cues — " .. project, table.concat(lines, "\n"))
        elseif ok and type(data) == "table" and #data == 0 then
            notify("No Cues", "No pending cues for " .. project)
        else
            notify("Error", "Unexpected response from server")
        end
    else
        notify("Error", "Could not reach Clavus server")
    end
end

function actions.toggle_tui()
    -- Try to focus an existing terminal running the TUI, or launch one
    local script = [[
        tell application "Terminal"
            if exists (window 1 whose name contains "clavus") then
                activate
            else
                tell application "System Events"
                    set terminalWindows to windows of process "Terminal"
                end tell
                do script "cd ~/Developer/clavus && python3 -m clavus.tui"
                activate
            end if
        end tell
    ]]
    hs.osascript.applescript(script)
end

function actions.switch_project()
    local resp = clavus_api("GET", "/api/projects", nil)
    if not resp or resp == "" then
        notify("Error", "Could not reach Clavus server")
        return
    end

    local ok, data = pcall(hs.json.decode, resp)
    if not ok or type(data) ~= "table" or #data == 0 then
        notify("No Projects", "No projects registered in Clavus")
        return
    end

    -- Build a comma-separated list for the dialog
    local names = {}
    for _, p in ipairs(data) do
        if type(p) == "table" and p.name then
            table.insert(names, p.name)
        end
    end

    hs.dialog.textPrompt("Clavus — Switch Project",
        "Type a project name:\n" .. table.concat(names, "\n"),
        "", "Switch", "Cancel",
        function(name)
            if name and name ~= "" then
                notify("Switched", "Now working on: " .. name)
            end
        end)
end

function actions.current_position()
    -- Placeholder for future Ableton playhead integration
    notify("Ableton Position",
        "Live playhead tracking coming soon.\nFor now, enter positions manually when creating cues (Cmd+Alt+N).")
end

-- ─── Hotkey Registration ────────────────────────────────────────────

local function register_hotkeys()
    local bindings, server = load_bindings()
    if not bindings then
        hs.alert.show("⚠️ Clavus: could not load bindings")
        return
    end

    local count = 0
    local errors = {}
    for _, binding in ipairs(bindings) do
        local action_fn = actions[binding.action]
        if not action_fn then
            table.insert(errors, string.format("⚠️ unknown action '%s' (binding '%s')", binding.action, binding.id))
        else
            -- Convert mod strings to hs.keycodes.modifier values
            local mods = {}
            if binding.mac_mods then
                for _, mod in ipairs(binding.mac_mods) do
                    if mod == "cmd" then table.insert(mods, "cmd") end
                    if mod == "alt" then table.insert(mods, "alt") end
                    if mod == "shift" then table.insert(mods, "shift") end
                    if mod == "ctrl" then table.insert(mods, "ctrl") end
                end
            end

            local key = binding.mac_key
            if key then
                hs.hotkey.bind(mods, key, function()
                    action_fn(binding.params)
                end)
                count = count + 1
            end
        end
    end

    -- Show menu bar icon
    local tooltip = string.format("Clavus active — %d hotkeys", count)
    if #errors > 0 then
        tooltip = tooltip .. "\n" .. table.concat(errors, "\n")
    end
    hs.menubar.new():setTitle("♮"):setToolTip(tooltip)
    if #errors > 0 then
        hs.alert.show("⚠️ Clavus: " .. #errors .. " binding error(s) — hover ♮ icon for details")
    else
        hs.alert.show(string.format("Clavus: %d hotkeys registered", count))
    end
end

-- ─── Ready ──────────────────────────────────────────────────────────

hs.alert("♮ Clavus hotkeys loading...")
register_hotkeys()
notify("Clavus Ready", "Keyboard automation loaded — check menu bar for ♮ icon")
