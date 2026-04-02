import sys
import subprocess
import os

def main():
    if len(sys.argv) < 2:
        print("Usage: python runner.py <executable> [args...]")
        sys.exit(1)

    executable = sys.argv[1]
    args = sys.argv[2:]

    if os.name == 'nt':
        # Windows
        # We use powershell to run the .ps1 script
        cmd = ["powershell", "-ExecutionPolicy", "Bypass", "-File", "run_qemu.ps1", executable] + args
    else:
        # Linux/Unix
        # We use bash to run the .sh script
        cmd = ["bash", "run_qemu.sh", executable] + args
    
    try:
        return subprocess.call(cmd)
    except Exception as e:
        print(f"Error executing runner: {e}")
        return 1

if __name__ == "__main__":
    sys.exit(main())
