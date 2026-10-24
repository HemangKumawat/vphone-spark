// FirmwarePipelineRestoreTXM.swift — patch the restore ramdisk's release TXM.
//
// vphone-linux: the restore boot chain loads `Firmware/txm.iphoneos.release.im4p`
// (manifest component `Ap,RestoreTrustedExecutionMonitor`), a separate, much
// smaller image from the guest's research TXM. Its trustcache binary-search /
// selector24 hash-flags call must return success for the ramdisk's ad-hoc
// signed binaries; the JB TXM patch set is for the research image and is not
// applied here. The old ramdisk_send.sh flow calls this the "base TXM patch (1)".

import Foundation
import Img4tool

extension FirmwarePipeline {
    /// Apply the base TXM trustcache bypass to the release (restore) TXM at `url`.
    /// Returns the number of patches written (0 when the site is already patched).
    @discardableResult
    public func patchRestoreTXMFile(at url: URL) throws -> Int {
        let image = try IM4PHandler.load(contentsOf: url)
        let patcher = TXMPatcher(data: image.payload, verbose: verbose)
        _ = try patcher.findAll()
        let count = try patcher.apply()
        guard count > 0 else { return 0 }
        try IM4PHandler.save(patchedData: patcher.patchedData, originalIM4P: image.im4p, to: url)
        return count
    }

    /// Force the selector-24 pre-check walk (`sub_30BAC` on 187.100.3) to its
    /// PASS exit.  This is the documented fix for `0x32202`/`0x12491` when
    /// the TXM trust-cache magazine is empty; it covers the ad-hoc and CMS
    /// fallbacks as well.  Only this patch is applied, not the whole dev set.
    @discardableResult
    public func patchRestoreTXMPrecheckFile(at url: URL) throws -> Int {
        let image = try IM4PHandler.load(contentsOf: url)
        let patcher = TXMDevPatcher(data: image.payload, verbose: verbose)
        // Force the selector24 handler itself to return PASS before the ladder
        // runs, and force the pre-check walk's PASS exit as a second layer.
        patcher.patchSelector24ForcePass()
        patcher.patchPrecheckForcePass()
        guard !patcher.patches.isEmpty else { return 0 }
        let count = try patcher.apply()
        try IM4PHandler.save(patchedData: patcher.patchedData, originalIM4P: image.im4p, to: url)
        return count
    }
}
