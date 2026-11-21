# Lucia의 Bash 명령 앞에 실행되는 환경(결정 0059). Claude Code가 CLAUDE_ENV_FILE로 읽는다.
# 이 파일은 읽기 전용 마운트다(VM 안에서 고칠 수 없다). 보호 영역: 사용자가 요청할 때만 바꾼다.
#
# - 데몬이 이 실행에 인터넷 통행증을 주었으면(LUCIA_PROXY) 쉘 명령이 데몬의 출구 프록시로 나가게 한다.
#   Claude Code 자신은 이 값을 쓰지 않는다(모델 API는 sbx의 프록시로 간다).
# - 통행증이 없으면 아무것도 바꾸지 않는다(바깥은 막혀 있다).
if [ -f /etc/sandbox-persistent.sh ]; then
  . /etc/sandbox-persistent.sh
fi
if [ -n "${LUCIA_PROXY:-}" ]; then
  export HTTP_PROXY="$LUCIA_PROXY" HTTPS_PROXY="$LUCIA_PROXY" ALL_PROXY="$LUCIA_PROXY"
  export http_proxy="$LUCIA_PROXY" https_proxy="$LUCIA_PROXY" all_proxy="$LUCIA_PROXY"
  export NO_PROXY="localhost,127.0.0.1,::1" no_proxy="localhost,127.0.0.1,::1"
fi
