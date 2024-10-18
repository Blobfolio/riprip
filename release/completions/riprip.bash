_basher___riprip() {
	local cur prev opts
	COMPREPLY=()
	cur="${COMP_WORDS[COMP_CWORD]}"
	prev="${COMP_WORDS[COMP_CWORD-1]}"
	opts=()
	[[ " ${COMP_LINE} " =~ " --backwards " ]] || opts+=("--backwards")
	[[ " ${COMP_LINE} " =~ " --flip-flop " ]] || opts+=("--flip-flop")
	if [[ ! " ${COMP_LINE} " =~ " -h " ]] && [[ ! " ${COMP_LINE} " =~ " --help " ]]; then
		opts+=("-h")
		opts+=("--help")
	fi
	[[ " ${COMP_LINE} " =~ " --no-resume " ]] || opts+=("--no-resume")
	[[ " ${COMP_LINE} " =~ " --no-rip " ]] || opts+=("--no-rip")
	[[ " ${COMP_LINE} " =~ " --no-summary " ]] || opts+=("--no-summary")
	[[ " ${COMP_LINE} " =~ " --reset " ]] || opts+=("--reset")
	[[ " ${COMP_LINE} " =~ " --status " ]] || opts+=("--status")
	[[ " ${COMP_LINE} " =~ " --strict " ]] || opts+=("--strict")
	[[ " ${COMP_LINE} " =~ " sync " ]] || opts+=("sync")
	if [[ ! " ${COMP_LINE} " =~ " -v " ]] && [[ ! " ${COMP_LINE} " =~ " --verbose " ]]; then
		opts+=("-v")
		opts+=("--verbose")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -V " ]] && [[ ! " ${COMP_LINE} " =~ " --version " ]]; then
		opts+=("-V")
		opts+=("--version")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -c " ]] && [[ ! " ${COMP_LINE} " =~ " --cache " ]]; then
		opts+=("-c")
		opts+=("--cache")
	fi
	[[ " ${COMP_LINE} " =~ " --confidence " ]] || opts+=("--confidence")
	if [[ ! " ${COMP_LINE} " =~ " -d " ]] && [[ ! " ${COMP_LINE} " =~ " --dev " ]]; then
		opts+=("-d")
		opts+=("--dev")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -o " ]] && [[ ! " ${COMP_LINE} " =~ " --offset " ]]; then
		opts+=("-o")
		opts+=("--offset")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -p " ]] && [[ ! " ${COMP_LINE} " =~ " --passes " ]]; then
		opts+=("-p")
		opts+=("--passes")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -r " ]] && [[ ! " ${COMP_LINE} " =~ " --rereads " ]]; then
		opts+=("-r")
		opts+=("--rereads")
	fi
	opts+=("-t")
	opts+=("--tracks")
	opts=" ${opts[@]} "
	if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
		COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
		return 0
	fi
	case "${prev}" in
		--dev|-d)
			if [ -z "$( declare -f _filedir )" ]; then
				COMPREPLY=( $( compgen -f "${cur}" ) )
			else
				COMPREPLY=( $( _filedir ) )
			fi
			return 0
			;;
		*)
			COMPREPLY=()
			;;
	esac
	COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
	return 0
}
complete -F _basher___riprip -o bashdefault -o default riprip
