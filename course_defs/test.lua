function init()
    define_grid(3,3)
    define_target('Target1', 'ipsc', 1, 1)
    define_distance("3ft.", 1,2)
    define_target('Target2', 'ipsc', 1,3)
    define_distance("3ft.", 2,3)
    define_target('Target3', 'ipsc', 3,3)
end

function main()
    start_timer("stage", "00:05.000")
    activate_targets(true)
    while timer_expired("stage") == false do
        process_targets()
    end
    activate_targets(false)
end

function activate_targets(state)
    for _, name in ipairs(global_targets.index) do
        global_targets[name]:active(state)
    end
end
