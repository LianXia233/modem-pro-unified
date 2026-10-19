#!/bin/sh
# 仓库脱敏守门：扫描**实机现场值**，防止真实地址 / 凭据重新进入仓库。
#
# 用法：sh scripts/check-desensitize.sh [目录，默认仓库根]
# 退出码：0 = 干净；1 = 发现可疑项（打印文件:行号 + 原因）
#
# 口径见 docs/PRIVACY.md。要点：
#   · 只允许**文档保留段**与**私有段**出现具体地址：
#       文档段：192.0.2.0/24、198.51.100.0/24、203.0.113.0/24、2001:db8::/32
#       私有段：10/8、172.16/12、192.168/16、127/8、169.254/16
#   · 历史实机网段（虽私有，但暴露用户网络拓扑）逐个列在下面；
#   · 运营商公网 IPv6 前缀（240e / 2409 / 2408 / 2400）一律不许出现；
#   · 凭据：ghp_ / github_pat_ / sk- / ssh 口令 / 真实穿透域名。
set -u
ROOT=${1:-$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)}
cd "$ROOT" || exit 2

FILES=$(find . -type f \
  \( -name '*.rs' -o -name '*.md' -o -name '*.toml' -o -name '*.sh' \
     -o -name '*.html' -o -name '*.js' -o -name '*.json' -o -name '*.uc' \) \
  -not -path './.git/*' -not -path './rust/target/*' \
  -not -path './scripts/*' 2>/dev/null)

fail=0
report() { fail=1; printf '\033[31m[脱敏]\033[0m %s\n' "$1"; printf '%s\n' "$2" | sed 's/^/    /'; }

# 1) 历史实机网段（私有地址，但能反推用户拓扑）
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '192\.168\.88\.|10\.4\.83\.|10\.161\.|10\.2\.98\.|10\.5\.23\.|10\.149\.|10\.7\.45\.' 2>/dev/null)
[ -n "$hits" ] && report "出现历史实机网段（示例请用 10.42.x.x / 192.168.1.x）" "$hits"

# 2) 运营商公网 IPv6 前缀
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b(240e|2409|2408|2400):' 2>/dev/null)
[ -n "$hits" ] && report "出现运营商公网 IPv6（示例请用 2001:db8::/32）" "$hits"

# 3) 合法但非文档段的公网 IPv4（逐段判 <=255，避免把 999.1.1.1、1.2.3.256 这类脏值当地址）
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' 2>/dev/null \
  | grep -vE '([0-9]{1,3}\.){7,}[0-9]{1,3}' \
  | grep -vE '5\.4\.2\.1|5\.6\.7\.8|114\.114\.114\.114|223\.5\.5\.5|1\.2\.3\.4' \
  | while IFS= read -r line; do
      bad=$(printf '%s\n' "$line" | grep -oE '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' | while IFS=. read -r a b c d; do
        [ "$a" -le 255 ] 2>/dev/null && [ "$b" -le 255 ] 2>/dev/null \
          && [ "$c" -le 255 ] 2>/dev/null && [ "$d" -le 255 ] 2>/dev/null || continue
        case "$a" in
          0|10|127) continue ;;
          255) continue ;;                      # 掩码
        esac
        case "$a.$b" in
          169.254|192.0|198.51|198.18|203.0|192.168|172.1[6-9]|172.2[0-9]|172.3[01]) continue ;;
          1.2) continue ;;                      # 测试用的「随便一个地址」
        esac
        echo "$a.$b.$c.$d"
      done | head -1)
      [ -n "$bad" ] && printf '%s\n' "$line"
    done)
[ -n "$hits" ] && report "出现非文档段公网 IPv4（示例请用 192.0.2.0/24 / 198.51.100.0/24 / 203.0.113.0/24）" "$hits"

# 4) 凭据与真实穿透域名（拆开写，避免脚本命中自己）
hits=$(printf '%s\n' "$FILES" | xargs grep -nEi \
  'ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9]{20,}|sshpass[[:space:]]+-p|frp-[a-z]+\.(com|net|cn)' 2>/dev/null)
[ -n "$hits" ] && report "出现凭据 / 真实穿透域名 / 登录口令" "$hits"

# 5) 真实手机号（示例请用 13800138000）
hits=$(printf '%s\n' "$FILES" | xargs grep -nE '\b1[3-9][0-9]{9}\b' 2>/dev/null | grep -v '13800138000')
[ -n "$hits" ] && report "出现疑似真实手机号" "$hits"

if [ "$fail" -eq 0 ]; then
  printf '\033[32m[脱敏]\033[0m 未发现实机现场值（%s 个文件已扫）\n' "$(printf '%s\n' "$FILES" | wc -l | tr -d ' ')"
fi
exit "$fail"
