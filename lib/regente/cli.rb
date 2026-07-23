# frozen_string_literal: true

require "optparse"

module Regente
  # Command-line entry. Subcommands:
  #   regente [task]            launch the master (in tmux) to work on a task
  #   regente mcp-serve --repo  run the MCP server on stdio (the master calls this)
  #   regente doctor            health-check the roster + show config
  #   regente attach            attach to the running master tmux session
  #   regente version           print version
  class CLI
    MASTER_SESSION = "regente-master"

    def initialize(stdin: $stdin, stdout: $stdout, stderr: $stderr,
                   home: Dir.home, cwd: Dir.pwd, launcher: nil, probe_runner: nil)
      @stdin = stdin
      @stdout = stdout
      @stderr = stderr
      @home = home
      @cwd = cwd
      @launcher = launcher || method(:default_launcher)
      @probe_runner = probe_runner
    end

    def run(argv)
      cmd = argv.first
      case cmd
      when "mcp-serve" then cmd_serve(argv[1..])
      when "doctor"    then cmd_doctor
      when "config"    then cmd_config(argv[1..])
      when "attach"    then cmd_attach
      when "version", "-v", "--version" then @stdout.puts("regente #{VERSION}"); 0
      when "help", "-h", "--help", nil then usage; 0
      else cmd_launch(argv)
      end
    rescue Regente::Error => e
      @stderr.puts("erro: #{e.message}")
      1
    end

    private

    def config
      Config.load(project_dir: git_repo? ? @cwd : nil, home: @home)
    end

    def cmd_serve(args)
      repo = parse_repo(args) || @cwd
      session = Session.new(repo: repo, config: Config.load(project_dir: repo, home: @home))
      MCP::Server.new(session: session, input: @stdin, output: @stdout).run
      0
    end

    def cmd_doctor
      cfg = config
      unless git_repo?
        @stdout.puts("aviso: #{@cwd} nao e repo git — modo 1-agente (sem worktree).")
      end
      engine = Engine.new(repo: @cwd, config: cfg, probe_runner: @probe_runner)
      @stdout.puts("mestre: #{cfg.master['cli']} (#{cfg.master['model']})")
      @stdout.puts("health check:")
      engine.health_check.each do |cli, ok|
        @stdout.puts("  #{ok ? '✓' : '✗'} #{cli}")
      end
      0
    end

    def cmd_config(args)
      sub = args.first
      cfg = config
      case sub
      when "get"
        @stdout.puts(cfg.get(args[1]).inspect)
      when "set"
        key = args[1]
        raw = args[2..].join(" ")
        cfg.set(key, coerce(raw))
        cfg.save_project
        @stdout.puts("#{key} = #{cfg.get(key).inspect}")
      when "show", nil
        @stdout.puts(YAML.dump(cfg.data))
      else
        @stderr.puts("uso: regente config [get KEY | set KEY VALUE | show]")
        return 1
      end
      0
    end

    # cast common scalars so `config set timeouts.worker 120` stores an Integer
    def coerce(str)
      case str
      when /\A-?\d+\z/ then str.to_i
      when /\A(true|false)\z/ then str == "true"
      else str
      end
    end

    def cmd_attach
      @launcher.call(["tmux", "attach", "-t", MASTER_SESSION], @cwd)
      0
    end

    def cmd_launch(argv)
      task = argv.join(" ").strip
      task = nil if task.empty?
      cfg = config
      prompt = Playbook.prompt(cfg)
      master = cfg.master
      cmd = Master.launch_string(cli: master["cli"], repo: @cwd, prompt: prompt,
                                 model: master["model"], task: task, exe: exe_path)
      # run the master inside a persistent tmux session for remote attach.
      @launcher.call(["tmux", "new-session", "-A", "-s", MASTER_SESSION,
                      "-c", @cwd, cmd], @cwd)
      0
    end

    def usage
      @stdout.puts(<<~USAGE)
        regente — orquestrador multi-agente de IAs

        uso:
          regente "<tarefa>"     comanda o mestre pra resolver a tarefa (em tmux)
          regente attach         entra na sessao do mestre (local ou via ssh)
          regente doctor         checa os bots do roster + mostra config
          regente config ...     get KEY | set KEY VALUE | show
          regente mcp-serve      (interno) servidor MCP no stdio
          regente version
      USAGE
    end

    def parse_repo(args)
      repo = nil
      OptionParser.new do |o|
        o.on("--repo PATH") { |v| repo = v }
      end.parse(args || [])
      repo
    end

    def git_repo?
      system("git", "-C", @cwd, "rev-parse", "--git-dir",
             out: File::NULL, err: File::NULL)
    end

    def exe_path
      "regente"
    end

    def default_launcher(argv, _repo)
      exec(*argv)
    end
  end
end
