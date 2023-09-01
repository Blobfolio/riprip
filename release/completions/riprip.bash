_basher___riprip() {
	local cur prev opts
	COMPREPLY=()
	cur="${COMP_WORDS[COMP_CWORD]}"
	prev="${COMP_WORDS[COMP_CWORD-1]}"
	opts=()
	[[ " ${COMP_LINE} " =~ " --clean " ]] || opts+=("--clean")
	if [[ ! " ${COMP_LINE} " =~ " -h " ]] && [[ ! " ${COMP_LINE} " =~ " --help " ]]; then
		opts+=("-h")
		opts+=("--help")
	fi
	[[ " ${COMP_LINE} " =~ " --no-c2 " ]] || opts+=("--no-c2")
	[[ " ${COMP_LINE} " =~ " --no-rip " ]] || opts+=("--no-rip")
	if [[ ! " ${COMP_LINE} " =~ " -V " ]] && [[ ! " ${COMP_LINE} " =~ " --version " ]]; then
		opts+=("-V")
		opts+=("--version")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -d " ]] && [[ ! " ${COMP_LINE} " =~ " --dev " ]]; then
		opts+=("-d")
		opts+=("--dev")
	fi
	if [[ ! " ${COMP_LINE} " =~ " -o " ]] && [[ ! " ${COMP_LINE} " =~ " --offset " ]]; then
		opts+=("-o")
		opts+=("--offset")
	fi
	[[ " ${COMP_LINE} " =~ " --paranoia " ]] || opts+=("--paranoia")
	if [[ ! " ${COMP_LINE} " =~ " -p " ]] && [[ ! " ${COMP_LINE} " =~ " --passes " ]]; then
		opts+=("-p")
		opts+=("--passes")
	fi
	opts+=("-t")
	opts+=("--track")
	opts=" ${opts[@]} "
	if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
		COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
		return 0
	fi
	case "${prev}" in
		-d|--dev)
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
