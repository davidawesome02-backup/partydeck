// KWin Script: Multi-Monitor Dynamic Window Resizer via D-Bus

var layoutService = "com.partydeck.layoutManager";
var layoutPath = "/com/partydeck/layoutManager";
var layoutInterface = "com.partydeck.layoutManager";
var layoutMethod = "ProcessLayout";

/**
 * Main entry point to trigger the layout logic.
 */
function applyLayout() {
    var allClients = workspace.windowList();
    
    // 1. Group windows by their output (monitor)
    var monitorWindowsMap = {};

    for (var i = 0; i < allClients.length; i++) {
        var client = allClients[i];
        if (!client || !client.pid || !client.output) {
            continue;
        }

        var monitor = client.output;
        var monitorId = monitor.name || monitor.id; // Use a unique identifier for the monitor

        if (!monitorWindowsMap[monitorId]) {
            monitorWindowsMap[monitorId] = {
                monitor: monitor,
                pids: [],
                windows: [] // Keep reference to the actual window objects to apply geometry later
            };
        }

        monitorWindowsMap[monitorId].pids.push(Number(client.pid-0));
        monitorWindowsMap[monitorId].windows.push(client);
    }

    var monitors = Object.keys(monitorWindowsMap);
    
    // If there are no windows, we can stop or just return. 
    // Note: We iterate through monitors, so if there are monitors but no windows, 
    // the map will be empty.
    if (monitors.length === 0) {
        return;
    }

    // 2. Process each monitor independently
    // We use a counter to track how many D-Bus calls are pending if you want strict ordering,
    // but since each call handles its own monitor's windows, they are independent.
    
    monitors.forEach(function(monitorId) {
        var data = monitorWindowsMap[monitorId];
        var monitor = data.monitor;
        var pids = data.pids;
        var windows = data.windows;

        // Prepare arguments for the D-Bus call:
        // 1. Monitor Width
        // 2. Monitor Height
        // 3. List of PIDs for this monitor
        var monWidth = monitor.geometry.width;
        var monHeight = monitor.geometry.height;
        var monX = monitor.geometry.x;
        var monY = monitor.geometry.y;

        print(monWidth);
        print(monHeight);
        print(pids);
        // callDataProcessor();
        // Call D-Bus
        callDBus(
            // layoutService,
            // layoutPath,
            // layoutInterface,
            // layoutMethod,

            "com.partydeck.layoutManager",        // service
            "/com/partydeck/layoutManager",       // path
            "com.partydeck.layoutManager",        // interface
            "ProcessLayout",                   // method

            Number(monWidth),
            Number(monHeight),
            pids, // Vec<i32>
            // 1920,
            // 1080,
            // [1234],

            function(result) {
                // Callback for this specific monitor
                if (!result || !Array.isArray(result)) {
                    print(result)
                    console.warn("D-Bus call failed or invalid result for monitor " + monitorId);
                    return;
                }

                // The result should have the same length as the pids sent
                if (result.length !== pids.length) {
                    print(result)

                    console.warn("D-Bus result length mismatch for monitor " + monitorId);
                    return;
                }

                // Apply geometry to each window
                for (var i = 0; i < windows.length; i++) {
                    var client = windows[i];
                    var layoutData = result[i];

                    // layoutData is expected to be [x, y, w, h]
                    if (layoutData && layoutData.length >= 4) {
                        var relX = layoutData[0];
                        var relY = layoutData[1];
                        var newW = layoutData[2];
                        var newH = layoutData[3];

                        // Check if size is 0 (skip)
                        if (newW === 0 || newH === 0) {
                            continue;
                        }

                        // Calculate absolute position by adding monitor offset
                        var absX = monX + relX;
                        var absY = monY + relY;

                        try {
                            client.frameGeometry = {
                                x: absX,
                                y: absY,
                                width: newW,
                                height: newH
                            };
                        } catch (e) {
                            console.error("Failed to set geometry for PID " + client.pid + ": " + e);
                        }
                    }
                }
            }
        );
    });
}

function callDataProcessor() {
    var inputNumbers = [1237546,2323]; // Vec<i32>
    var width = 1920;
    var height = 1080;

    callDBus(
        "com.partydeck.layoutManager",        // service
        "/com/partydeck/layoutManager",       // path
        "com.partydeck.layoutManager",        // interface
        "ProcessLayout",                   // method
        Number(width),
        Number(height),
        inputNumbers,                       // argument (Vec<i32>)
    function(result) {
        // result is an array of arrays: [[1, 2, 3, 4], [2, 4, 6, 8], ...]
        console.log("D-Bus call succeeded!");
        console.log("Input:", inputNumbers);
        console.log("Output:", result);

        // Process the result
        if (result && Array.isArray(result)) {
            result.forEach(function(quad, index) {
                console.log(
                    "Number " + inputNumbers[index] +
                    " -> (" + quad[0] + ", " + quad[1] +
                    ", " + quad[2] + ", " + quad[3] + ")"
                );
            });
        }
    }
    );
}

// Connect signals
// Using windowAdded and windowRemoved is crucial to catch when windows appear/disappear.
// windowActivated is less critical for layout changes but harmless.
workspace.windowAdded.connect(applyLayout);
workspace.windowRemoved.connect(applyLayout);
workspace.windowActivated.connect(applyLayout);





// // function getGamescopeClients() {
  
// //   var gamescopeClients = [];

// //   for (var i = 0; i < allClients.length; i++) {
// //     if (
// //       allClients[i].resourceClass == "gamescope" ||
// //       allClients[i].resourceClass == "gamescope-kbm"
// //     ) {
// //       gamescopeClients.push(allClients[i]);
// //     }
// //   }
// //   return gamescopeClients;
// // }


// function gamescopeSplitscreen() {

//   print("asd");
  
//   var allClients = workspace.windowList();
//   for (var i = 0; i < allClients.length; i++) {
//     print(allClients[i].pid);
//   }

//   function callDataProcessor() {
//     var inputNumbers = [1237546]; // Vec<i32>
//     var width = 1920;
//     var height = 1080;

//     callDBus(
//         "com.partydeck.layoutManager",        // service
//         "/com/partydeck/layoutManager",       // path
//         "com.partydeck.layoutManager",        // interface
//         "ProcessLayout",                   // method
//         Number(width),
//         Number(height),
//         inputNumbers,                       // argument (Vec<i32>)
//         function(result) {
//             // result is an array of arrays: [[1, 2, 3, 4], [2, 4, 6, 8], ...]
//             console.log("D-Bus call succeeded!");
//             console.log("Input:", inputNumbers);
//             console.log("Output:", result);

//             // Process the result
//             if (result && Array.isArray(result)) {
//                 result.forEach(function(quad, index) {
//                     console.log(
//                         "Number " + inputNumbers[index] +
//                         " -> (" + quad[0] + ", " + quad[1] +
//                         ", " + quad[2] + ", " + quad[3] + ")"
//                     );
//                 });
//             }
//         }
//         );
//   }

//   // var gamescopeClients = getGamescopeClients();

//   // var screenMap = new Map();
//   // var screens = workspace.screens;
//   // for (var j = 0; j < screens.length; j++) {
//   //   screenMap.set(screens[j], 0);
//   // }

//   // for (var i = 0; i < gamescopeClients.length; i++) {
//   //   var monitor = gamescopeClients[i].output;
//   //   var monitorX = monitor.geometry.x;
//   //   var monitorY = monitor.geometry.y;
//   //   var monitorWidth = monitor.geometry.width;
//   //   var monitorHeight = monitor.geometry.height;

//   //   var playerCount = numGamescopeClientsInOutput(monitor);
//   //   var playerIndex = screenMap.get(monitor);
//   //   screenMap.set(monitor, playerIndex + 1);

//   //   gamescopeClients[i].noBorder = true;
//   //   gamescopeClients[i].frameGeometry = {
//   //     x: monitorX + x[playerCount][playerIndex] * monitorWidth,
//   //     y: monitorY + y[playerCount][playerIndex] * monitorHeight,
//   //     width: monitorWidth * width[playerCount][playerIndex],
//   //     height: monitorHeight * height[playerCount][playerIndex],
//   //   };
//   // }
//   // gamescopeAboveBelow();
// }

// workspace.windowAdded.connect(gamescopeSplitscreen);
// workspace.windowRemoved.connect(gamescopeSplitscreen);
// workspace.windowActivated.connect(gamescopeSplitscreen);
