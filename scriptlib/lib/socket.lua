-- LuaSocket-compatible stubs. Host passes sleep and gettime as args.
local host_sleep, host_gettime = ...
local socket = {}

function socket.sleep(n)
  host_sleep(n)
end

function socket.gettime()
  return host_gettime()
end

return socket
