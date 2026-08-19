#!/usr/bin/env node
/**
 * verify-no-any.mjs
 * 
 * AST-based TypeScript verifier that fails on explicit `any` keywords.
 * Uses the TypeScript compiler API to parse project-owned .ts and .tsx files
 * and reports any SyntaxKind.AnyKeyword occurrences.
 */

import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const desktopDirectory = path.resolve(scriptDirectory, "..");
const sourceDirectory = path.join(desktopDirectory, "src");

// Files/directories to exclude from scanning
const excludedPatterns = [
  /node_modules/,
  /\.d\.ts$/,
  /vite-env\.d\.ts$/,
];

function fail(message) {
  console.error(`\n❌ No-any verification failed:\n${message}\n`);
  process.exit(1);
}

function shouldExclude(filePath) {
  return excludedPatterns.some((pattern) => pattern.test(filePath));
}

async function collectTypeScriptFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      // Skip node_modules and other excluded directories
      if (!shouldExclude(entryPath)) {
        files.push(...(await collectTypeScriptFiles(entryPath)));
      }
    } else if (entry.isFile() && !shouldExclude(entryPath)) {
      if (entry.name.endsWith(".ts") || entry.name.endsWith(".tsx")) {
        files.push(entryPath);
      }
    }
  }

  return files;
}

function findAnyKeywords(sourceFile, filePath) {
  const violations = [];
  
  function visit(node) {
    // Check for AnyKeyword in type annotations
    if (node.kind === ts.SyntaxKind.AnyKeyword) {
      const { line, character } = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
      violations.push({
        file: filePath,
        line: line + 1,
        column: character + 1,
        text: node.getText(sourceFile).trim(),
      });
    }
    
    ts.forEachChild(node, visit);
  }
  
  visit(sourceFile);
  return violations;
}

async function verifyNoAny() {
  console.log("🔍 Scanning for explicit 'any' keywords in TypeScript sources...\n");
  
  const files = await collectTypeScriptFiles(sourceDirectory);
  console.log(`Found ${files.length} TypeScript files to scan.\n`);
  
  const allViolations = [];
  
  for (const filePath of files) {
    const content = await readFile(filePath, "utf-8");
    const sourceFile = ts.createSourceFile(
      filePath,
      content,
      ts.ScriptTarget.Latest,
      true, // setParentNodes
      filePath.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS
    );
    
    const violations = findAnyKeywords(sourceFile, filePath);
    allViolations.push(...violations);
  }
  
  if (allViolations.length > 0) {
    const report = allViolations
      .map((v) => `  ${path.relative(desktopDirectory, v.file)}:${v.line}:${v.column} - found '${v.text}'`)
      .join("\n");
    
    fail(`Found ${allViolations.length} explicit 'any' keyword(s):\n${report}`);
  }
  
  console.log("✅ No explicit 'any' keywords found in project-owned TypeScript sources.");
  process.exit(0);
}

verifyNoAny().catch((error) => {
  console.error("Verification error:", error);
  process.exit(1);
});
