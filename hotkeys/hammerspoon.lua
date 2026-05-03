-- Clavus Hotkeys
-- Using AppleScript for curl (io.popen is blocked by sandbox)
-- Ctrl+Shift+letter — works inside Ableton

local function notify(title, msg)
    hs.notify.new({title="Clavus", subTitle=title, informativeText=msg}):send()
end

-- Run curl via AppleScript's do shell script
local function api(method, path, body)
    local url = "http://127.0.0.1:7890" .. path
    local script
    if body then
        -- Write body to temp file, then curl
        script = [[
            do shell script "echo ']] .. body .. [[' > /tmp/clavus_body.json && curl -s -X ]] .. method .. [[ --max-time 3 -H 'Content-Type: application/json' -d @/tmp/clavus_body.json ']] .. url .. [['"
        ]]
    else
        script = [[
            do shell script "curl -s -X ]] .. method .. [[ --max-time 3 ']] .. url .. [['"
        ]]
    end
    local ok, result = hs.osascript.applescript(script)
    if ok then
        return result
    end
    return nil
end

-- ─── Quick Cue (Ctrl+Shift+N) ─────────────────────────
hs.hotkey.bind({"ctrl", "shift"}, "N", function()
    hs.dialog.textPrompt("Clavus — New Cue", "Cue text:", "", "Next", "Cancel",
        function(text)
            if not text or text == "" then return end
            hs.dialog.textPrompt("Clavus — Position", "Position @ (blank=1.1.1):", "", "Add", "Cancel",
                function(pos)
                    if not pos or pos == "" then pos = "1.1.1" end
                    local resp = api("GET", "/api/projects")
                    if not resp then notify("Error", "Start clavus serve first") return end
                    local _, _, project = resp:find('"name"%s*:%s*"([^"]+)"')
                    if not project then notify("Error", "No project found") return end
                    local body = '{"text":"' .. text .. '","position":"' .. pos .. '","author":"hammerspoon","project_name":"' .. project .. '"}'
                    local cresp = api("POST", "/api/cues", body)
                    if cresp then
                        notify("Cue Added", '"' .. text .. '" @ ' .. pos)
                    else
                        notify("Error", "Could not reach Clavus server")
                    end
                end)
        end)
end)

-- ─── List Cues (Ctrl+Shift+L) ─────────────────────────
hs.hotkey.bind({"ctrl", "shift"}, "L", function()
    local resp = api("GET", "/api/projects")
    if not resp then notify("Error", "Start clavus serve first") return end
    local _, _, project = resp:find('"name"%s*:%s*"([^"]+)"')
    if not project then notify("Error", "No project") return end
    local cresp = api("GET", "/api/cues?name=" .. project .. "&limit=5&status=pending")
    if cresp then
        local lines = {}
        for text in cresp:gmatch('"text"%s*:%s*"([^"]+)"') do
            table.insert(lines, "• " .. text)
        end
        if #lines > 0 then
            notify("Recent Cues", table.concat(lines, "\n"))
        else
            notify("No Cues", "No pending cues")
        end
    else
        notify("Error", "Could not reach Clavus server")
    end
end)

-- ─── Inject Markers (Ctrl+Shift+I) ────────────────────
hs.hotkey.bind({"ctrl", "shift"}, "I", function()
    local resp = api("GET", "/api/projects")
    if not resp then notify("Error", "Start clavus serve first") return end
    local _, _, project = resp:find('"name"%s*:%s*"([^"]+)"')
    if not project then notify("Error", "No project") return end
    local cresp = api("POST", "/api/projects/inject?name=" .. project)
    if cresp then
        local _, _, count = cresp:find('"injected"%s*:%s*([0-9]+)')
        if count then
            notify("Cues Injected", count .. " cue(s) written to " .. project)
        else
            local _, _, msg = cresp:find('"message"%s*:%s*"([^"]+)"')
            notify("Injection", msg or cresp)
        end
    else
        notify("Error", "Could not reach Clavus server")
    end
end)

-- ─── Toggle TUI (Ctrl+Shift+T) ───────────────────────
hs.hotkey.bind({"ctrl", "shift"}, "T", function()
    hs.osascript.applescript([[
        tell application "Terminal"
            do script "cd ~/Developer/clavus && python3 -m clavus.tui"
            activate
        end tell
    ]])
end)

-- ─── Switch Project (Ctrl+Shift+P) ───────────────────
hs.hotkey.bind({"ctrl", "shift"}, "P", function()
    local resp = api("GET", "/api/projects")
    if not resp then notify("Error", "No server") return end
    local names = ""
    for name in resp:gmatch('"name"%s*:%s*"([^"]+)"') do
        names = names .. name .. "\n"
    end
    if names == "" then notify("No Projects", "None registered") return end
    hs.dialog.textPrompt("Switch Project", "Available:\n" .. names, "", "Switch", "Cancel",
        function(name)
            if name and name ~= "" then notify("Switched", "Now on: " .. name) end
        end)
end)

-- ─── Where Am I? (Ctrl+Shift+W) ──────────────────────
hs.hotkey.bind({"ctrl", "shift"}, "W", function()
    notify("Position", "Playhead tracking coming soon.")
end)

-- ─── Ready! ──────────────────────────────────────────
hs.alert.show("♮ Clavus: 6 hotkeys (AppleScript curl)")
