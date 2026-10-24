// vphone-linux overlay of vphone-cli's main.swift: no boot hand-off (QEMU boots the VM).
import ArgumentParser
import Foundation

signal(SIGPIPE, SIG_IGN)
VPhoneCommand.main()
