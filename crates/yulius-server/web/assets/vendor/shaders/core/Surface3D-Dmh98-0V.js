import { $c as WAVE_HALFEXTENT, Ac as defineStd, Bd as ZERO, Cs as recompileWhen, Dc as vec3, Gs as clamp, Hd as call, Jc as CURSOR_WAVE_BOUND, Nd as createPingPongPair, Oc as vec4, Qc as WAVE_GRID, Xc as HEIGHT_SLAB_PAD, Yc as HEIGHT_FIELD_STATE_FORMAT, Zc as WAVE_DECAY, al as makeHeightFieldMarchKernel, dc as mul, el as WAVE_SETTLE_MS, fd as isMobileGpuViewport, if as unpremultiplyAlpha, il as heightFieldMarchLayout, mf as DEG_TO_RAD$1, nl as createKernelPass, oc as local, ol as makeWaveEquationKernel, rl as createWaveStateBuffers, sl as waveEquationLayout, tl as createHeightFieldParamsUniform } from "./std-ClftfWpF.js";
import { a as transformColor, s as transformEdges } from "./transformations-P50Uo_Ju.js";
var COMPUTE_MAX = isMobileGpuViewport() ? 960 : 1600;
var COMPUTE_FALLBACK_W = 1600;
var COMPUTE_FALLBACK_H = 900;
var MARCH_STEPS = isMobileGpuViewport() ? 12 : 16;
var MARCH_REFINE = 6;
var DEG_TO_RAD = DEG_TO_RAD$1;
const componentDefinition = defineStd({
	name: "Surface3D",
	role: "shapeEffect",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Distortions",
	description: "Drapes child content over a 3D wave surface with perspective and lighting",
	requiresRTT: true,
	requiresChild: true,
	usesPointer: true,
	providesUVContextViaCompute: true,
	props: {
		amplitude: {
			default: .3,
			description: "Wave height",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Amplitude",
				group: "Waves"
			}
		},
		waveType: {
			default: "fractal",
			compileTime: true,
			description: "Wave pattern type",
			ui: {
				type: "select",
				options: [
					{
						label: "Fractal",
						value: "fractal"
					},
					{
						label: "Sine",
						value: "sine"
					},
					{
						label: "Ridge",
						value: "ridge"
					}
				],
				label: "Wave Type",
				group: "Waves"
			}
		},
		frequency: {
			default: 1.5,
			description: "Wave frequency",
			ui: {
				type: "range",
				min: .1,
				max: 5,
				step: .1,
				label: "Frequency",
				group: "Waves"
			}
		},
		octaves: {
			default: 2,
			recompile: recompileWhen((prev, next) => Math.max(1, Math.floor(prev)) !== Math.max(1, Math.floor(next))),
			description: "Detail octaves",
			ui: {
				type: "range",
				min: 1,
				max: 5,
				step: 1,
				label: "Octaves",
				group: "Waves"
			}
		},
		seed: {
			default: 0,
			description: "Random seed offset",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Waves"
			}
		},
		speed: {
			default: .5,
			description: "Animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 3,
				step: .1,
				label: "Speed",
				group: "Waves"
			}
		},
		tilt: {
			default: 35,
			description: "Camera tilt in degrees",
			ui: {
				type: "range",
				min: 0,
				max: 85,
				step: 1,
				label: "Tilt",
				group: "Camera"
			}
		},
		roll: {
			default: 0,
			description: "Camera roll in degrees",
			ui: {
				type: "range",
				min: -45,
				max: 45,
				step: 1,
				label: "Roll",
				group: "Camera"
			}
		},
		height: {
			default: 0,
			description: "Camera height",
			ui: {
				type: "range",
				min: -1,
				max: 2,
				step: .01,
				label: "Height",
				group: "Camera"
			}
		},
		zoom: {
			default: 1,
			description: "Camera zoom",
			ui: {
				type: "range",
				min: .3,
				max: 3,
				step: .01,
				label: "Zoom",
				group: "Camera"
			}
		},
		nearCutoff: {
			default: 0,
			description: "Near fade distance",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Near Cutoff",
				group: "Camera"
			}
		},
		farCutoff: {
			default: 1,
			description: "Far fade distance",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Far Cutoff",
				group: "Camera"
			}
		},
		edgePinning: {
			default: 0,
			description: "Smoothly flatten the surface at its edges (orthogonal to edge mode)",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Edge Pinning",
				group: "Waves"
			}
		},
		edges: {
			default: "mirror",
			transform: transformEdges,
			compileTime: true,
			description: "How to handle content beyond the surface bounds",
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
				group: "Waves"
			}
		},
		lighting: {
			default: 30,
			recompile: recompileWhen((prev, next) => prev < .01 !== next < .01),
			description: "Intensity of lighting and shading",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Lighting",
				group: "Lighting"
			}
		},
		glossiness: {
			default: 0,
			description: "Surface glossiness",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Glossiness",
				group: "Lighting"
			}
		},
		highlights: {
			default: 15,
			description: "Specular highlight intensity",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Highlights",
				group: "Lighting"
			}
		},
		lightX: {
			default: .4,
			description: "Light direction X",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Light X",
				group: "Lighting"
			}
		},
		lightY: {
			default: -.6,
			description: "Light direction Y",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Light Y",
				group: "Lighting"
			}
		},
		lightZ: {
			default: .7,
			description: "Light direction Z",
			ui: {
				type: "range",
				min: -1,
				max: 1,
				step: .01,
				label: "Light Z",
				group: "Lighting"
			}
		},
		lightColor: {
			default: "#ffffff",
			transform: transformColor,
			description: "Light color",
			ui: {
				type: "color",
				label: "Light Color",
				group: "Lighting"
			}
		},
		cursorIntensity: {
			default: 1,
			recompile: recompileWhen((prev, next) => prev < .01 !== next < .01),
			description: "Strength of cursor ripples",
			ui: {
				type: "range",
				min: 0,
				max: 2,
				step: .01,
				label: "Cursor Ripples",
				group: "Interaction"
			}
		},
		cursorSpeed: {
			default: .5,
			description: "Speed of cursor ripple propagation",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Ripple Speed",
				group: "Interaction"
			}
		}
	},
	compute: (params) => {
		const { gpu, getCpuValue, registerComputeTexture, onCleanup, onResize, dimensions } = params;
		const root = gpu?.root;
		if (!root) return null;
		const waveType = getCpuValue("waveType") === "sine" ? 1 : getCpuValue("waveType") === "ridge" ? 2 : 0;
		const octaveCount = Math.max(1, Math.floor(getCpuValue("octaves") ?? 2));
		const edgeMode = getCpuValue("edges") ?? 2;
		const lightingEnabled = (getCpuValue("lighting") ?? 30) >= .01;
		const cursorEnabled = (getCpuValue("cursorIntensity") ?? 1) >= .01;
		const canvasW = Math.round(dimensions.width);
		const canvasH = Math.round(dimensions.height);
		let computeW = Math.min(COMPUTE_MAX, COMPUTE_FALLBACK_W);
		let computeH = Math.min(Math.round(COMPUTE_MAX * COMPUTE_FALLBACK_H / COMPUTE_FALLBACK_W), COMPUTE_FALLBACK_H);
		if (canvasW >= 16 && canvasH >= 16) {
			const fit = Math.min(1, COMPUTE_MAX / Math.max(canvasW, canvasH));
			computeW = Math.max(16, Math.round(canvasW * fit));
			computeH = Math.max(16, Math.round(canvasH * fit));
		}
		const uvMaskTex = root.createTexture({
			size: [computeW, computeH],
			format: HEIGHT_FIELD_STATE_FORMAT
		}).$usage("storage", "sampled");
		const litTex = root.createTexture({
			size: [computeW, computeH],
			format: HEIGHT_FIELD_STATE_FORMAT
		}).$usage("storage", "sampled");
		onCleanup(() => {
			uvMaskTex.destroy();
			litTex.destroy();
		});
		const uvMaskMap = registerComputeTexture(uvMaskTex);
		const litMap = registerComputeTexture(litTex);
		const { bufferA, bufferB, readBuf } = createWaveStateBuffers(root);
		const paramsU = createHeightFieldParamsUniform(root);
		let curW = Math.max(1, Math.round(dimensions.width));
		let curH = Math.max(1, Math.round(dimensions.height));
		onResize(({ width, height }) => {
			curW = Math.max(1, Math.round(width));
			curH = Math.max(1, Math.round(height));
		});
		const computeKernel = makeHeightFieldMarchKernel({
			waveType,
			octaveCount,
			edgeMode,
			lightingEnabled,
			cursorEnabled,
			marchSteps: MARCH_STEPS,
			marchRefine: MARCH_REFINE,
			computeW,
			computeH
		});
		const computeBg = root.createBindGroup(heightFieldMarchLayout, {
			readBuf,
			params: paramsU.buffer,
			uvMaskTex,
			litTex
		});
		const computePass = createKernelPass(root, computeKernel, {
			size: [computeW, computeH],
			bindGroup: computeBg
		});
		const propagate = cursorEnabled ? makeWaveEquationKernel() : null;
		const wave = createPingPongPair(bufferA, bufferB);
		const propGroup = cursorEnabled ? wave.groups((readSrc, writeBuf) => root.createBindGroup(waveEquationLayout, {
			readSrc,
			writeBuf,
			readBuf,
			params: paramsU.buffer
		})) : null;
		const propagatePass = cursorEnabled && propagate ? createKernelPass(root, propagate, { size: [256, 256] }) : null;
		let prevPx = .5;
		let prevPy = .5;
		let lastTime = Date.now();
		let simSinceActive = 0;
		let animT = 0;
		let lastSig = null;
		const writeParams = (fp, mouseSpeed, cursorActive) => {
			const aspect = Math.max(1, curW) / Math.max(1, curH);
			const tilt = (getCpuValue("tilt") ?? 35) * DEG_TO_RAD;
			const roll = (getCpuValue("roll") ?? 0) * DEG_TO_RAD;
			const camHeight = getCpuValue("height") ?? 0;
			const zoom = getCpuValue("zoom") ?? 1;
			const { camPos, fwd, camRight, camUp, focal } = resolveOrbitCamera(tilt, roll, camHeight, zoom);
			const px = fp.pointer?.x ?? .5;
			const py = fp.pointer?.y ?? .5;
			const cNdcX = (px - .5) * 2 * aspect;
			const cNdcY = -(py - .5) * 2;
			const rd = norm3([
				fwd[0] * focal + camRight[0] * cNdcX + camUp[0] * cNdcY,
				fwd[1] * focal + camRight[1] * cNdcX + camUp[1] * cNdcY,
				fwd[2] * focal + camRight[2] * cNdcX + camUp[2] * cNdcY
			]);
			const cRayDownZ = Math.max(-rd[2], .001);
			const cT0 = camPos[2] / cRayDownZ;
			const cWX0 = camPos[0] + rd[0] * cT0;
			const cWY0 = camPos[1] + rd[1] * cT0;
			const cursorWorldUVx = cWX0 * .5 + .5;
			const cursorWorldUVy = cWY0 * .5 + .5;
			const lc = getCpuValue("lightColor");
			const amp = getCpuValue("amplitude") ?? .3;
			const cursorIntensity = getCpuValue("cursorIntensity") ?? 1;
			const freq = getCpuValue("frequency") ?? 1.5;
			const seed = getCpuValue("seed") ?? 0;
			const edgePin = getCpuValue("edgePinning") ?? 0;
			const lighting = (getCpuValue("lighting") ?? 30) * .035;
			const glossiness = (getCpuValue("glossiness") ?? 0) * .01;
			const highlights = (getCpuValue("highlights") ?? 15) * .04;
			const lightX = getCpuValue("lightX") ?? .4;
			const lightY = getCpuValue("lightY") ?? -.6;
			const lightZ = getCpuValue("lightZ") ?? .7;
			const nearCutoff = getCpuValue("nearCutoff") ?? 0;
			const farCutoff = getCpuValue("farCutoff") ?? 1;
			const waveSpeed = getCpuValue("cursorSpeed") ?? .5;
			const cursorWaveX = (cursorWorldUVx - .5) / 3 + .5;
			const cursorWaveY = (cursorWorldUVy - .5) / 3 + .5;
			const heightEnvelope = amp + HEIGHT_SLAB_PAD + (cursorActive > 0 ? cursorIntensity * .05 * 6 : 0);
			paramsU.write({
				amp,
				freq,
				seed,
				edgePin,
				cursorIntensity,
				t: animT,
				aspect,
				focal,
				camPos,
				camForward: fwd,
				camRight,
				camUp,
				lighting,
				glossiness,
				highlights,
				lightX,
				lightY,
				lightZ,
				lightColor: [
					lc?.x ?? 1,
					lc?.y ?? 1,
					lc?.z ?? 1
				],
				nearCutoff,
				farCutoff,
				cursorWaveX,
				cursorWaveY,
				mouseSpeed,
				dt: .016,
				decay: 10,
				radius: .025,
				waveSpeed,
				heightEnvelope,
				cursorActive
			});
			return [
				amp,
				freq,
				seed,
				edgePin,
				cursorIntensity,
				animT,
				aspect,
				tilt,
				roll,
				camHeight,
				zoom,
				lighting,
				glossiness,
				highlights,
				lightX,
				lightY,
				lightZ,
				lc?.x ?? 1,
				lc?.y ?? 1,
				lc?.z ?? 1,
				nearCutoff,
				farCutoff,
				cursorWaveX,
				cursorWaveY,
				mouseSpeed,
				waveSpeed,
				heightEnvelope,
				cursorActive
			].join(",");
		};
		return {
			outputs: {
				uvMaskMap,
				litMap
			},
			getComputeNodes: (frameParams) => {
				const fp = frameParams;
				const now = Date.now();
				const dt = Math.min(fp.deltaTime ?? (now - lastTime) / 1e3, .016);
				lastTime = now;
				const px = fp.pointer?.x ?? prevPx;
				const py = fp.pointer?.y ?? prevPy;
				const rawVelX = dt > 0 ? (px - prevPx) / dt : 0;
				const rawVelY = dt > 0 ? (py - prevPy) / dt : 0;
				const mouseSpeed = Math.min(Math.sqrt(rawVelX * rawVelX + rawVelY * rawVelY), 2);
				prevPx = px;
				prevPy = py;
				if (mouseSpeed > .01) simSinceActive = 0;
				else simSinceActive += dt;
				animT += dt * (getCpuValue("speed") ?? .5);
				const waveSettled = !cursorEnabled || !propagatePass || simSinceActive * 1e3 > WAVE_SETTLE_MS && mouseSpeed < .01;
				const sig = writeParams(fp, mouseSpeed, waveSettled ? 0 : 1);
				if (waveSettled && sig === lastSig) return [];
				lastSig = sig;
				if (waveSettled || !propagatePass || !propGroup) return [computePass];
				const prop = propagatePass.with(propGroup());
				wave.swap();
				return [prop, computePass];
			}
		};
	},
	gpu: { fragment: (params) => {
		const { childNode, ctx, computeOutputs, convertToTexture } = params;
		if (!childNode) return ZERO;
		const uvMaskTex = computeOutputs?.uvMaskMap;
		const litTex = computeOutputs?.litMap;
		if (!uvMaskTex || !litTex) return ZERO;
		const uvMaskSample = local(uvMaskTex.sample(ctx.uv, "linearClamp"), "uvMaskSample");
		const litSample = litTex.sample(ctx.uv, "linearClamp");
		const lookedUpUV = vec4(uvMaskSample.member("r"), uvMaskSample.member("g"), 0, 0).member("xy");
		const mask = uvMaskSample.member("b");
		const lit = litSample.member("rgb");
		const childColor = local(params.uvPropagationActive ? childNode : convertToTexture(childNode).sample(lookedUpUV), "childColor");
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [vec4(clamp(mul(childColor.member("xyz"), lit), vec3(0, 0, 0), vec3(1, 1, 1)), mul(childColor.member("w"), mask))]);
	} }
});
function cross3(a, b) {
	return [
		a[1] * b[2] - a[2] * b[1],
		a[2] * b[0] - a[0] * b[2],
		a[0] * b[1] - a[1] * b[0]
	];
}
function norm3(a) {
	const l = Math.max(1e-6, Math.hypot(a[0], a[1], a[2]));
	return [
		a[0] / l,
		a[1] / l,
		a[2] / l
	];
}
function resolveOrbitCamera(tilt, roll, camHeight, zoom) {
	const camDist = .85 / Math.max(1e-4, zoom);
	const camY = -Math.sin(tilt) * camDist;
	const orbitZ = Math.cos(tilt) * camDist;
	const camPos = [
		0,
		camY,
		orbitZ + camHeight
	];
	const fwd = norm3([
		0,
		-camY,
		-orbitZ
	]);
	const right0 = [
		1,
		0,
		0
	];
	const up0 = norm3(cross3(right0, fwd));
	const cr = Math.cos(roll);
	const sr = Math.sin(roll);
	return {
		camPos,
		fwd,
		camRight: [
			right0[0] * cr + up0[0] * sr,
			right0[1] * cr + up0[1] * sr,
			right0[2] * cr + up0[2] * sr
		],
		camUp: [
			up0[0] * cr - right0[0] * sr,
			up0[1] * cr - right0[1] * sr,
			up0[2] * cr - right0[2] * sr
		],
		focal: 1.5
	};
}
var Surface3D_default = componentDefinition;
export { componentDefinition as n, Surface3D_default as t };
