// vphone-linux overlay (new file): `fw snapshot-flip`.
//
// The macOS installer ends a full `cfw install` by renaming the guest's APFS
// root snapshot (`com.apple.os.update-<hash>`) offline on Disk.img, so the
// seal-enforcement-patched kernel cannot find the named root snapshot and roots
// the live System volume — which is where the CFW install wrote the cryptexes,
// the dyld cache and the patched binaries. Without it the kernel boots the old
// sealed snapshot: launchd then panics with
// "Library not loaded: /usr/lib/libSystem.B.dylib ... (no dyld cache)".
//
// On Linux the guest, not the host, holds the System volume during `cfw
// install`, so this is a separate step, run with the VM stopped. It is the same
// VPhoneAPFSSnapshot the macOS installer uses (a same-length b-tree rename with
// the blocks' fletcher64 checksums updated).
import ArgumentParser
import Foundation
import VPhoneCoreKit

struct VPhoneFirmwareSnapshotFlipCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "snapshot-flip",
        abstract: "Rename the guest's APFS root snapshot so it roots the live System volume",
        discussion: """
        Run after `cfw install` (a full install), with the VM stopped. Booting a VM whose \
        System volume was modified without this flip roots the old sealed snapshot, and \
        launchd panics on a missing dyld cache.
        """,
    )

    @OptionGroup var lib: VPhoneLibraryOption
    @Argument(help: "VM name") var name: String?
    @Flag(help: "Report what would be renamed, without writing") var dryRun = false

    func run() throws {
        let vmName = try VPhoneVirtualMachineSelection.resolveExisting(name, in: lib.library)
        let bundle = try lib.library.bundle(named: vmName)
        let image = bundle.url.appendingPathComponent("Disk.img")
        guard FileManager.default.fileExists(atPath: image.path) else {
            throw ValidationError("No Disk.img in \(bundle.url.path).")
        }
        if !dryRun, isRunning(bundle.url) {
            throw ValidationError("\(vmName) is running. Stop it before flipping the snapshot.")
        }
        let report = try VPhoneAPFSSnapshot.rename(imageAt: image, dryRun: dryRun) { print($0) }
        if report.isEmpty {
            print("[snapshot-flip] no com.apple.os.update-* snapshot found; nothing to flip")
            return
        }
        print("[snapshot-flip] \(dryRun ? "would rename" : "renamed") "
            + "\(report.snapshotName ?? "the root snapshot") "
            + "(\(report.recordCount) record(s) in \(report.blocks.count) block(s))")
    }

    /// A running QEMU holds the image; writing it under the guest would corrupt
    /// either side's view of the APFS b-tree.
    private func isRunning(_ url: URL) -> Bool {
        let output = (try? VPhoneProcessRunner.runCapturing(
            URL(fileURLWithPath: "/usr/bin/pgrep"), ["-af", url.path],
        ))?.stdout ?? ""
        return output.contains("qemu-system")
    }
}
