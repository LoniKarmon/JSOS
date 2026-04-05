// PXE Configuration Analysis Library
// Parses and audits Windows PXE boot configurations (BCD files)

const PXE = {
    // BCD Object Type Identifiers (GUIDs)
    BCD_OBJECT_TYPES: {
        FIRMWARE_BOOT_MANAGER: '{9dea862c-5cdd-4e70-acc1-f32b344370f5}',
        WINDOWS_BOOT_MANAGER: '{45f8d5de-e803-4d60-b3c8-f7971043def9}',
        WINDOWS_BOOT_LOADER: '{ae5534e0-a924-466b-9287-d71d86d176d7}',
        RESUME_LOADER: '{1ade1cb0-eba1-11da-a3dd-0800200c9a66}',
        MEMORY_DIAGNOSTICS: '{5fb23c6d-cdee-4d2d-add9-98f83a3e37f9}',
        DEBUG_OPTION: '{4636856e-540f-4170-a130-a84776f4c654}',
    },

    // BCD Element Types (common ones)
    BCD_ELEMENTS: {
        // Boot manager elements
        APPLICATION_PATH: 0x12000004,
        DESCRIPTION: 0x12000003,
        INHERIT_DEFAULT_LOCALE: 0x15000001,
        RECOVERYSEQUENCE: 0x14000009,
        TIMEOUT: 0x14000008,
        DISPLAYORDER: 0x1400000a,
        CURRENT: 0x1400000b,

        // Boot loader elements
        SYSTEM_ROOT: 0x22000002,
        OSDEVICE: 0x21000001,
        RESUME_OBJECT: 0x21000003,
        LOAD_OPTIONS: 0x12000030,
        KERNEL_PATH: 0x12000021,
        RAMDISK_PATH: 0x12000023,
        RAMDISK_OPTIONS: 0x12000031,

        // Debug elements
        DEBUG_ENABLED: 0x16000010,
        KERNEL_DEBUG_ENABLED: 0x16000011,
        EMS_ENABLED: 0x16000012,
        SECURE_BOOT: 0x16000013,
    },

    /**
     * Parse BCD binary data from store
     * @param {string} storeKey - Key to retrieve from os.store (e.g., 'pxe:bcd')
     * @returns {Object} Parsed BCD structure with entries and GUIDs
     */
    parseBCD(storeKey) {
        try {
            // Try to get bytes from store
            const dataStr = os.store.getBytes(storeKey);
            if (!dataStr) {
                return { error: 'Key not found', key: storeKey, entries: [], guids: [] };
            }

            // Convert string to Uint8Array
            const data = new Uint8Array(dataStr.length);
            for (let i = 0; i < dataStr.length; i++) {
                data[i] = dataStr.charCodeAt(i) & 0xFF;
            }

            const result = {
                key: storeKey,
                isRegistryHive: false,
                hiveVersion: 0,
                entries: [],
                guids: [],
                elements: [],
                rawSize: data.length,
                errors: [],
            };

            // Check for registry hive magic
            if (data.length >= 4) {
                const magic = String.fromCharCode(data[0], data[1], data[2], data[3]);
                result.hiveSignature = magic;
                result.isRegistryHive = (magic === 'regf');
            }

            // Scan for GUIDs (pattern: {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx})
            // GUID in binary is 16 bytes; in text form with braces is 38 chars
            result.guids = this._extractGUIDs(dataStr);

            // Scan for common element types and values
            result.elements = this._extractElements(data);

            // Parse known BCD structures if it's a valid hive
            if (result.isRegistryHive) {
                result.entries = this._parseHiveEntries(data);
            }

            return result;
        } catch (e) {
            return {
                error: 'Parse failed: ' + (e.message || e),
                key: storeKey,
                entries: [],
                guids: [],
            };
        }
    },

    /**
     * Extract GUIDs from data (text pattern matching)
     * @private
     */
    _extractGUIDs(dataStr) {
        const guids = [];
        // Match GUID pattern: {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}
        const guidPattern = /\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\}/g;
        let match;
        const seenGuids = new Set();
        while ((match = guidPattern.exec(dataStr)) !== null) {
            const guid = match[0].toUpperCase();
            if (!seenGuids.has(guid)) {
                guids.push(guid);
                seenGuids.add(guid);
            }
        }
        return guids;
    },

    /**
     * Extract known element types from binary data
     * @private
     */
    _extractElements(data) {
        const elements = [];
        const foundElements = new Set();

        // Look for element type markers (little-endian 32-bit integers)
        for (let i = 0; i < data.length - 3; i++) {
            const val = data[i] | (data[i+1] << 8) | (data[i+2] << 16) | (data[i+3] << 24);

            // Check if this matches a known element type
            for (const [name, type] of Object.entries(this.BCD_ELEMENTS)) {
                if (val === type && !foundElements.has(type)) {
                    elements.push({ name, type: '0x' + type.toString(16), offset: i });
                    foundElements.add(type);
                }
            }
        }

        return elements;
    },

    /**
     * Parse registry hive entries (simplified)
     * @private
     */
    _parseHiveEntries(data) {
        const entries = [];

        // Simple hive parsing: look for key name structures
        // Registry key names follow patterns; we'll look for known boot entry patterns

        // Look for boot manager entry pattern
        if (data.length > 100) {
            // Hive header is at offset 0
            const hiveVersion = (data[24] | (data[25] << 8) | (data[26] << 16) | (data[27] << 24));
            entries.push({
                type: 'Hive Metadata',
                hiveVersion: hiveVersion,
                offset: 0,
            });
        }

        // Look for root key node
        for (let i = 0; i < data.length - 3; i++) {
            // Root key block signature: 'nk' (0x6b6e) at offset 4 in a key node
            if (data[i] === 0x6b && data[i+1] === 0x6e) {
                entries.push({
                    type: 'Key Node',
                    offset: i - 4,
                    signature: 'nk',
                });
                break;
            }
        }

        return entries;
    },

    /**
     * Format parsed BCD for human-readable display
     * @param {Object} bcd - Parsed BCD object from parseBCD()
     * @returns {string} Formatted text
     */
    formatBCD(bcd) {
        let output = '';
        output += '=== BCD Configuration ===\n';
        output += `Store Key: ${bcd.key}\n`;
        output += `Raw Size: ${bcd.rawSize} bytes\n`;
        output += `Registry Hive: ${bcd.isRegistryHive ? 'YES (' + bcd.hiveSignature + ')' : 'NO'}\n`;

        if (bcd.error) {
            output += `Error: ${bcd.error}\n`;
            return output;
        }

        output += '\n=== GUIDs Found ===\n';
        if (bcd.guids.length > 0) {
            bcd.guids.forEach(guid => {
                output += `  ${guid}\n`;
                // Try to identify known types
                for (const [name, type] of Object.entries(this.BCD_OBJECT_TYPES)) {
                    if (guid === type) {
                        output += `    Type: ${name}\n`;
                        break;
                    }
                }
            });
        } else {
            output += '  (none found)\n';
        }

        output += '\n=== Elements Found ===\n';
        if (bcd.elements.length > 0) {
            bcd.elements.forEach(elem => {
                output += `  ${elem.name} (${elem.type}) @ offset ${elem.offset}\n`;
            });
        } else {
            output += '  (none found)\n';
        }

        output += '\n=== Entries ===\n';
        if (bcd.entries.length > 0) {
            bcd.entries.forEach(entry => {
                output += `  Type: ${entry.type}, Offset: ${entry.offset}\n`;
                if (entry.hiveVersion) {
                    output += `    Hive Version: ${entry.hiveVersion}\n`;
                }
                if (entry.signature) {
                    output += `    Signature: ${entry.signature}\n`;
                }
            });
        } else {
            output += '  (none parsed)\n';
        }

        return output;
    },

    /**
     * Security audit of BCD configuration
     * @param {string} serverIp - Server IP address for TFTP context
     * @param {Object} bcd - Parsed BCD object
     * @returns {Object} Audit findings
     */
    audit(serverIp, bcd) {
        const findings = {
            timestamp: new Date().toISOString(),
            serverIp: serverIp,
            severity: 'INFO',
            checks: [],
            warnings: [],
            criticals: [],
        };

        // Check 1: TFTP Access
        findings.checks.push({
            name: 'TFTP Server Accessibility',
            passed: true,
            description: `BCD may be accessed via TFTP from ${serverIp}`,
            recommendation: 'Restrict TFTP access to authorized networks',
        });

        // Check 2: Debug enabled detection
        if (bcd.elements && bcd.elements.length > 0) {
            const hasDebug = bcd.elements.some(e =>
                e.name === 'DEBUG_ENABLED' || e.name === 'KERNEL_DEBUG_ENABLED'
            );

            if (hasDebug) {
                findings.criticals.push({
                    name: 'Debug Mode Enabled',
                    severity: 'CRITICAL',
                    description: 'Kernel debugging is enabled in boot configuration',
                    recommendation: 'Disable debugging in production environments',
                });
                findings.severity = 'CRITICAL';
            } else {
                findings.checks.push({
                    name: 'Debug Mode',
                    passed: true,
                    description: 'No kernel debug elements detected',
                });
            }
        }

        // Check 3: EMS enabled detection
        if (bcd.elements && bcd.elements.length > 0) {
            const hasEMS = bcd.elements.some(e => e.name === 'EMS_ENABLED');

            if (hasEMS) {
                findings.warnings.push({
                    name: 'EMS (Emergency Management Services) Enabled',
                    severity: 'WARNING',
                    description: 'EMS is enabled for remote console access',
                    recommendation: 'Verify EMS credentials and access controls',
                });
                if (findings.severity === 'INFO') {
                    findings.severity = 'WARNING';
                }
            }
        }

        // Check 4: Secure Boot status
        if (bcd.elements && bcd.elements.length > 0) {
            const hasSecureBoot = bcd.elements.some(e => e.name === 'SECURE_BOOT');

            if (!hasSecureBoot) {
                findings.warnings.push({
                    name: 'Secure Boot Status Unknown',
                    severity: 'WARNING',
                    description: 'Secure Boot configuration not detected',
                    recommendation: 'Verify Secure Boot is enabled for production',
                });
                if (findings.severity === 'INFO') {
                    findings.severity = 'WARNING';
                }
            } else {
                findings.checks.push({
                    name: 'Secure Boot',
                    passed: true,
                    description: 'Secure Boot element detected',
                });
            }
        }

        // Check 5: Unsigned images detection
        if (bcd.guids && bcd.guids.length > 0) {
            // GUIDs alone don't indicate signing, but presence of boot manager is good
            const hasBootManager = bcd.guids.some(g =>
                g === this.BCD_OBJECT_TYPES.WINDOWS_BOOT_MANAGER
            );

            if (hasBootManager) {
                findings.checks.push({
                    name: 'Boot Manager Configuration',
                    passed: true,
                    description: 'Windows Boot Manager found in configuration',
                });
            }
        }

        // Check 6: TFTP ACL check
        findings.checks.push({
            name: 'TFTP Access Control',
            passed: false,
            description: 'No TFTP ACL information available from BCD',
            recommendation: 'Check TFTP server configuration for access restrictions',
        });

        // Check 7: Hive integrity
        if (!bcd.isRegistryHive) {
            findings.warnings.push({
                name: 'Registry Hive Format',
                severity: 'WARNING',
                description: 'Data does not appear to be a valid Windows Registry hive',
                recommendation: 'Verify BCD store format and integrity',
            });
            if (findings.severity === 'INFO') {
                findings.severity = 'WARNING';
            }
        }

        return findings;
    },

    /**
     * Format audit findings for display
     * @param {Object} findings - Audit findings from audit()
     * @returns {string} Formatted report
     */
    formatAudit(findings) {
        let output = '';
        output += '=== PXE Boot Configuration Audit ===\n';
        output += `Server: ${findings.serverIp}\n`;
        output += `Date: ${findings.timestamp}\n`;
        output += `Overall Severity: ${findings.severity}\n\n`;

        if (findings.criticals && findings.criticals.length > 0) {
            output += '🔴 CRITICAL FINDINGS\n';
            findings.criticals.forEach(f => {
                output += `  [${f.severity}] ${f.name}\n`;
                output += `    Issue: ${f.description}\n`;
                output += `    Fix: ${f.recommendation}\n\n`;
            });
        }

        if (findings.warnings && findings.warnings.length > 0) {
            output += '🟡 WARNINGS\n';
            findings.warnings.forEach(f => {
                output += `  [${f.severity}] ${f.name}\n`;
                output += `    Issue: ${f.description}\n`;
                output += `    Fix: ${f.recommendation}\n\n`;
            });
        }

        if (findings.checks && findings.checks.length > 0) {
            output += '✓ CHECKS\n';
            findings.checks.forEach(c => {
                const status = c.passed ? '✓' : '✗';
                output += `  [${status}] ${c.name}\n`;
                output += `    Result: ${c.description}\n`;
                if (!c.passed) {
                    output += `    Action: ${c.recommendation}\n`;
                }
                output += '\n';
            });
        }

        return output;
    },

    /**
     * Helper: Read and parse a BCD file in one call
     * @param {string} storeKey - Store key
     * @returns {Object} Combined result with parsed BCD and audit
     */
    analyze(storeKey, serverIp = '0.0.0.0') {
        const bcd = this.parseBCD(storeKey);
        const audit = this.audit(serverIp, bcd);

        return {
            bcd: bcd,
            audit: audit,
            summary: {
                key: storeKey,
                guids: bcd.guids.length,
                elements: bcd.elements.length,
                severity: audit.severity,
            },
        };
    },

    /**
     * Helper: Full analysis with formatted output
     * @param {string} storeKey - Store key
     * @param {string} serverIp - Server IP (optional)
     * @returns {string} Complete formatted report
     */
    report(storeKey, serverIp = '0.0.0.0') {
        const analysis = this.analyze(storeKey, serverIp);

        let output = '';
        output += this.formatBCD(analysis.bcd);
        output += '\n\n';
        output += this.formatAudit(analysis.audit);

        return output;
    },
};

// Export as module
if (typeof module !== 'undefined' && module.exports) {
    module.exports = PXE;
}

// Also make it global
globalThis.PXE = PXE;
